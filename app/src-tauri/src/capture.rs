use crate::native_process::SpawnTied;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{bounded, Receiver, Sender};

/// Audio buffers waiting for the archiver. Device callbacks deliver roughly
/// 10 ms each, so this rides out a stall of about 20 s (a slow disk sync or a
/// large save) before any audio is replaced by a recorded gap.
const FRAME_QUEUE: usize = 2048;
use std::{
    io::{Read, Write},
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
pub struct Frame {
    pub start: u64,
    pub samples: Vec<f32>,
}
pub struct CaptureHandle {
    pub stop: Arc<AtomicBool>,
    pub thread: Option<JoinHandle<()>>,
}
impl CaptureHandle {
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}
pub struct Source {
    pub frames: Receiver<Frame>,
    pub errors: Receiver<String>,
    pub gaps: Receiver<(u64, u64)>,
    pub notices: Receiver<MicrophoneNotice>,
    pub captured: Arc<AtomicU64>,
    pub rate: u32,
    pub handle: CaptureHandle,
}
/// How long a lost microphone may stay unavailable before the recording stops.
const RECONNECT_WINDOW: Duration = Duration::from_secs(60);

pub fn microphone() -> Result<Source, String> {
    let (tx, rx) = bounded(FRAME_QUEUE);
    let (etx, erx) = bounded(16);
    let (_gtx, grx) = bounded(1);
    let (ntx, nrx) = bounded(16);
    let (ready, rdy) = bounded(1);
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let captured = Arc::new(AtomicU64::new(0));
    let cursor = captured.clone();
    let thread = thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let host = cpal::default_host();
            let lost = Arc::new(AtomicBool::new(false));
            let (mut stream, name, rate) = open_input(&host, None, 0, &tx, &etx, &cursor, &lost)?;
            let _ = ready.send(Ok(rate));
            let mut current = name;
            loop {
                while !flag.load(Ordering::Relaxed) && !lost.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(30));
                }
                drop(stream);
                if flag.load(Ordering::Relaxed) {
                    return Ok(());
                }
                // The device went away mid-lecture: keep the timeline running and
                // pick up whichever input the system now offers.
                let _ = ntx.try_send(MicrophoneNotice::Lost(current.clone()));
                let since = std::time::Instant::now();
                let resumed = cursor.load(Ordering::Acquire);
                loop {
                    if flag.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    let elapsed = since.elapsed();
                    if elapsed > RECONNECT_WINDOW {
                        return Err(format!("麦克风“{current}”已断开，{} 秒内没有可用的输入设备", RECONNECT_WINDOW.as_secs()));
                    }
                    let skipped = (elapsed.as_secs_f64() * rate as f64) as u64;
                    lost.store(false, Ordering::Relaxed);
                    match open_input(&host, Some(rate), resumed + skipped, &tx, &etx, &cursor, &lost) {
                        Ok((next, name, _)) => {
                            let _ = ntx.try_send(MicrophoneNotice::Switched(name.clone()));
                            stream = next;
                            current = name;
                            break;
                        }
                        Err(_) => thread::sleep(Duration::from_millis(500)),
                    }
                }
            }
        })();
        if let Err(e) = result {
            let _ = ready.try_send(Err(e.clone()));
            let _ = etx.try_send(e);
        }
    });
    match rdy.recv_timeout(Duration::from_secs(25)) {
        Ok(Ok(rate)) => Ok(Source {
            frames: rx,
            errors: erx,
            gaps: grx,
            notices: nrx,
            captured,
            rate,
            handle: CaptureHandle {
                stop,
                thread: Some(thread),
            },
        }),
        r => {
            stop.store(true, Ordering::SeqCst);
            Err(match r {
                Ok(Err(e)) => e,
                _ => "麦克风未就绪，请在系统设置中允许“随堂”使用麦克风后重试".into(),
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MicrophoneNotice {
    Lost(String),
    Switched(String),
}

/// Opens the default input, then any other input if that fails. `rate` fixes
/// the timeline rate after a switch; a device running at another rate is
/// resampled to it so sample offsets stay continuous.
fn open_input(
    host: &cpal::Host,
    rate: Option<u32>,
    start: u64,
    tx: &Sender<Frame>,
    errors: &Sender<String>,
    cursor: &Arc<AtomicU64>,
    lost: &Arc<AtomicBool>,
) -> Result<(cpal::Stream, String, u32), String> {
    let mut candidates: Vec<cpal::Device> = host.default_input_device().into_iter().collect();
    let default_name = candidates.first().and_then(|device| device.name().ok());
    if let Ok(devices) = host.input_devices() {
        candidates.extend(devices.filter(|device| device.name().ok() != default_name));
    }
    if candidates.is_empty() {
        return Err("未找到麦克风，请连接输入设备".into());
    }
    let mut first_error = None;
    for device in candidates {
        match open_device(&device, rate, start, tx, errors, cursor, lost) {
            Ok(opened) => return Ok(opened),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    Err(first_error.unwrap_or_else(|| "无法打开麦克风".into()))
}

fn open_device(
    device: &cpal::Device,
    rate: Option<u32>,
    start: u64,
    tx: &Sender<Frame>,
    errors: &Sender<String>,
    cursor: &Arc<AtomicU64>,
    lost: &Arc<AtomicBool>,
) -> Result<(cpal::Stream, String, u32), String> {
    let name = device.name().unwrap_or_else(|_| "麦克风".into());
    let config = device
        .default_input_config()
        .map_err(|e| format!("无法打开麦克风：{e}"))?;
    let device_rate = config.sample_rate().0;
    let timeline_rate = rate.unwrap_or(device_rate);
    let channels = config.channels() as usize;
    let format = config.sample_format();
    let config: cpal::StreamConfig = config.into();
    let link = Link {
        tx: tx.clone(),
        errors: errors.clone(),
        cursor: cursor.clone(),
        lost: lost.clone(),
        start,
        channels,
        convert: RateConverter::new(device_rate, timeline_rate),
    };
    let stream = match format {
        cpal::SampleFormat::F32 => build::<f32>(device, &config, link),
        cpal::SampleFormat::I16 => build::<i16>(device, &config, link),
        cpal::SampleFormat::U16 => build::<u16>(device, &config, link),
        f => return Err(format!("麦克风采样格式暂未支持：{f:?}")),
    }?;
    stream.play().map_err(|e| format!("麦克风启动失败：{e}"))?;
    Ok((stream, name, timeline_rate))
}

struct Link {
    tx: Sender<Frame>,
    errors: Sender<String>,
    cursor: Arc<AtomicU64>,
    lost: Arc<AtomicBool>,
    start: u64,
    channels: usize,
    convert: RateConverter,
}

/// Linear-interpolating rate converter used only when a replacement device
/// runs at a different rate from the one the recording started with. Speech is
/// downsampled to 16 kHz afterwards, so its accuracy is ample.
pub(crate) struct RateConverter {
    step: f64,
    position: f64,
    previous: Option<f32>,
}

impl RateConverter {
    pub(crate) fn new(from: u32, to: u32) -> Self {
        Self {
            step: from as f64 / to.max(1) as f64,
            position: 0.0,
            previous: None,
        }
    }

    pub(crate) fn push(&mut self, input: Vec<f32>) -> Vec<f32> {
        if (self.step - 1.0).abs() < f64::EPSILON || input.is_empty() {
            return input;
        }
        // `position` counts from the previous block's last sample (index -1).
        let mut output = Vec::with_capacity((input.len() as f64 / self.step) as usize + 2);
        let previous = self.previous.unwrap_or(input[0]);
        let sample = |index: isize| if index < 0 { previous } else { input[index as usize] };
        let last = input.len() as f64 - 1.0;
        while self.position <= last {
            let base = self.position.floor();
            let fraction = (self.position - base) as f32;
            let left = sample(base as isize);
            let right = sample((base as isize + 1).min(last as isize));
            output.push(left + (right - left) * fraction);
            self.position += self.step;
        }
        self.position -= input.len() as f64;
        self.previous = input.last().copied();
        output
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    link: Link,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let Link {
        tx,
        errors,
        cursor: captured,
        lost,
        start,
        channels,
        mut convert,
    } = link;
    let mut cursor = start;
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                let mono: Vec<f32> = data
                    .chunks_exact(channels)
                    .map(|f| {
                        f.iter()
                            .map(|s| <f32 as cpal::FromSample<T>>::from_sample_(*s))
                            .sum::<f32>()
                            / channels as f32
                    })
                    .collect();
                let samples = convert.push(mono);
                let start = cursor;
                cursor += samples.len() as u64;
                captured.store(cursor, Ordering::Release);
                // A full queue loses this buffer only; absolute source offsets reveal the gap.
                let _ = tx.try_send(Frame { start, samples });
            },
            move |e| {
                // A vanished device is recovered by reopening; anything else is
                // reported as before.
                if matches!(e, cpal::StreamError::DeviceNotAvailable) {
                    lost.store(true, Ordering::Relaxed);
                } else {
                    let _ = errors.try_send(format!("音频设备中断：{e}"));
                }
            },
            None,
        )
        .map_err(|e| format!("无法采集音频：{e}"))
}

#[derive(Debug, PartialEq, Eq)]
enum HelperMessage {
    Ready,
    Gap(u64, u64),
    Fatal(String),
    Stopped,
}

fn parse_helper_message(line: &str) -> HelperMessage {
    if line == "READY" {
        return HelperMessage::Ready;
    }
    if line == "STOPPED" {
        return HelperMessage::Stopped;
    }
    if line.starts_with("ERROR:") || line.starts_with("DROP:") {
        return HelperMessage::Fatal(line.to_owned());
    }
    if let Some(value) = line.strip_prefix("GAP:") {
        let mut fields = value.split(':');
        let parsed = fields
            .next()
            .and_then(|start| start.parse::<u64>().ok())
            .zip(fields.next().and_then(|end| end.parse::<u64>().ok()));
        if let Some((start, end)) = parsed {
            if fields.next().is_none() && start < end {
                return HelperMessage::Gap(start, end);
            }
        }
    }
    HelperMessage::Fatal(format!("ERROR:PROTOCOL:unexpected helper message: {line}"))
}

fn helper_error_for_user(error: &str) -> String {
    if error.contains("SCStreamErrorDomain Code=-3801")
        || error.contains("SCStreamErrorUserDeclined")
        || error.contains("ERROR:SCREEN_PERMISSION:")
    {
        return "请在“系统设置”→“隐私与安全性”→“录屏与系统录音”中允许“随堂”，然后重新打开应用".into();
    }
    if error.starts_with("ERROR:WASAPI:") {
        return "系统声音采集启动失败，请检查播放设备后重试".into();
    }
    if error.starts_with("ERROR:PROTOCOL:") {
        return "系统声音组件响应异常，请重新打开应用后重试".into();
    }
    "系统声音采集失败，请重试".into()
}

#[derive(Default)]
struct PcmStreamDecoder {
    cursor: u64,
    tail: Option<u8>,
}

impl PcmStreamDecoder {
    fn push(&mut self, bytes: &[u8]) -> Option<Frame> {
        let mut data = Vec::with_capacity(bytes.len() + usize::from(self.tail.is_some()));
        if let Some(byte) = self.tail.take() {
            data.push(byte);
        }
        data.extend_from_slice(bytes);
        if data.len() % 2 != 0 {
            self.tail = data.pop();
        }
        if data.is_empty() {
            return None;
        }
        let samples: Vec<f32> = data
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32768.)
            .collect();
        let start = self.cursor;
        self.cursor += samples.len() as u64;
        Some(Frame { start, samples })
    }

    fn finish(&self) -> Result<u64, String> {
        if self.tail.is_some() {
            Err("ERROR:PROTOCOL:system audio helper ended with an incomplete PCM sample".into())
        } else {
            Ok(self.cursor)
        }
    }
}

pub fn system(helper: PathBuf) -> Result<Source, String> {
    if !helper.is_file() {
        return Err("当前安装缺少系统声音组件，请使用麦克风或安装完整桌面包".into());
    }
    let mut child = crate::native_process::command(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_tied()
        .map_err(|e| e.to_string())?;
    let mut out = child.stdout.take().ok_or("系统声音管道不可用")?;
    let err = child.stderr.take().ok_or("系统声音错误管道不可用")?;
    let (tx, rx) = bounded(FRAME_QUEUE);
    let (etx, erx) = bounded(16);
    let (gtx, grx) = bounded(256);
    let (ready, rdy) = bounded(1);
    let stderr_errors = etx.clone();
    thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
            match parse_helper_message(&line) {
                HelperMessage::Ready => {
                    let _ = ready.try_send(Ok(()));
                }
                HelperMessage::Gap(start, end) => {
                    let _ = gtx.send((start, end));
                }
                HelperMessage::Fatal(error) => {
                    eprintln!("system audio helper: {error}");
                    let message = helper_error_for_user(&error);
                    let _ = ready.try_send(Err(message.clone()));
                    let _ = stderr_errors.try_send(message);
                }
                HelperMessage::Stopped => {}
            }
        }
    });
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let captured = Arc::new(AtomicU64::new(0));
    let reader_cursor = captured.clone();
    let reader_errors = etx.clone();
    let frame_guard = tx.clone();
    let reader = thread::spawn(move || {
        let mut bytes = vec![0; 6400];
        let mut decoder = PcmStreamDecoder::default();
        while let Ok(n) = out.read(&mut bytes) {
            if n == 0 {
                break;
            }
            if let Some(frame) = decoder.push(&bytes[..n]) {
                reader_cursor.store(decoder.cursor, Ordering::Release);
                let _ = tx.try_send(frame);
            }
        }
        match decoder.finish() {
            Ok(cursor) => reader_cursor.store(cursor, Ordering::Release),
            Err(error) => {
                let _ = reader_errors.try_send(error);
            }
        }
    });
    let thread = thread::spawn(move || {
        let _keep_frames_open = frame_guard;
        while !flag.load(Ordering::Relaxed) {
            match child.try_wait() {
                Ok(Some(_)) => break,
                _ => thread::sleep(Duration::from_millis(30)),
            }
        }
        if let Some(mut input) = child.stdin.take() {
            let _ = input.write_all(b"\n");
        }
        for _ in 0..250 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
    });
    match rdy.recv_timeout(Duration::from_secs(25)) {
        Ok(Ok(())) => Ok(Source {
            frames: rx,
            errors: erx,
            gaps: grx,
            notices: bounded(1).1,
            captured,
            rate: 16000,
            handle: CaptureHandle {
                stop,
                thread: Some(thread),
            },
        }),
        r => {
            stop.store(true, Ordering::SeqCst);
            Err(match r {
                Ok(Err(e)) => e,
                _ if cfg!(windows) => "系统声音组件启动超时，请确认默认播放设备可用后重试".into(),
                _ => "请在系统设置中允许“随堂”录制系统声音后重试".into(),
            })
        }
    }
}
/// Streaming anti-alias low-pass with exact rational output cadence.
pub struct Resampler {
    rate: u32,
    phase: u64,
    ring: Vec<f32>,
    at: usize,
    taps: Vec<f32>,
}
impl Resampler {
    pub fn new(rate: u32) -> Self {
        let n = 63;
        let cutoff = (7600.0f64 / rate as f64).min(0.45);
        let mut taps: Vec<f32> = (0..n)
            .map(|i| {
                let x = i as f64 - (n - 1) as f64 / 2.;
                let sinc = if x == 0. {
                    2. * cutoff
                } else {
                    (2. * std::f64::consts::PI * cutoff * x).sin() / (std::f64::consts::PI * x)
                };
                let w = 0.54 - 0.46 * (2. * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos();
                (sinc * w) as f32
            })
            .collect();
        let sum: f32 = taps.iter().sum();
        for v in &mut taps {
            *v /= sum;
        }
        Self {
            rate,
            phase: 0,
            ring: vec![0.; n],
            at: 0,
            taps,
        }
    }
    pub fn push(&mut self, input: &[f32]) -> Vec<i16> {
        if self.rate == 16000 {
            return input
                .iter()
                .map(|v| (v.clamp(-1., 1.) * 32767.) as i16)
                .collect();
        }
        let mut out = Vec::new();
        for &v in input {
            self.ring[self.at] = v;
            self.at = (self.at + 1) % self.ring.len();
            self.phase += 16000;
            while self.phase >= self.rate as u64 {
                self.phase -= self.rate as u64;
                let mut x = 0.;
                for (i, t) in self.taps.iter().enumerate() {
                    x += self.ring[(self.at + i) % self.ring.len()] * t;
                }
                out.push((x.clamp(-1., 1.) * 32767.) as i16);
            }
        }
        out
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_protocol_is_strict_and_parses_gaps() {
        assert_eq!(parse_helper_message("READY"), HelperMessage::Ready);
        assert_eq!(
            parse_helper_message("GAP:160:320"),
            HelperMessage::Gap(160, 320)
        );
        assert!(matches!(
            parse_helper_message("READY-ish"),
            HelperMessage::Fatal(_)
        ));
        assert!(matches!(
            parse_helper_message("GAP:320:160"),
            HelperMessage::Fatal(_)
        ));
        assert!(matches!(
            parse_helper_message("GAP:1:2:3"),
            HelperMessage::Fatal(_)
        ));
    }

    #[test]
    fn helper_errors_are_safe_and_actionable_for_the_ui() {
        let raw = "ERROR:SHAREABLE_CONTENT:Error Domain=SCStreamErrorDomain Code=-3801 UserInfo={NSLocalizedDescription=The user declined}";
        let permission = helper_error_for_user(raw);
        assert!(permission.contains("录屏与系统录音"));
        assert!(permission.contains("重新打开应用"));
        assert!(!permission.contains("SCStreamErrorDomain"));

        let generic = helper_error_for_user("ERROR:START:a very large Cocoa error blob");
        assert_eq!(generic, "系统声音采集失败，请重试");
        assert!(!generic.contains("Cocoa"));
    }

    #[test]
    fn pcm_decoder_preserves_samples_across_arbitrary_splits() {
        let expected = [i16::MIN, -1234, -1, 0, 1, 1234, i16::MAX];
        let bytes: Vec<u8> = expected
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let mut decoder = PcmStreamDecoder::default();
        let mut decoded = Vec::new();
        let mut starts = Vec::new();
        let mut offset = 0;
        for size in [1, 4, 3, 2, 4] {
            if let Some(frame) = decoder.push(&bytes[offset..offset + size]) {
                starts.push(frame.start);
                decoded.extend(frame.samples);
            }
            offset += size;
        }
        assert_eq!(offset, bytes.len());
        assert_eq!(starts, vec![0, 2, 4, 5]);
        let round_trip: Vec<i16> = decoded
            .into_iter()
            .map(|sample| (sample * 32768.).round() as i16)
            .collect();
        assert_eq!(round_trip, expected);
        assert_eq!(decoder.finish().unwrap(), expected.len() as u64);
    }

    #[test]
    fn pcm_decoder_rejects_incomplete_final_sample() {
        let mut decoder = PcmStreamDecoder::default();
        assert!(decoder.push(&[0x12]).is_none());
        assert!(decoder.finish().unwrap_err().contains("incomplete PCM"));
    }

    #[test]
    fn streaming_length() {
        for rate in [16000, 44100, 48000] {
            let mut r = Resampler::new(rate);
            let mut count = 0;
            for chunk in vec![0.2; rate as usize * 2].chunks(997) {
                count += r.push(chunk).len();
            }
            assert_eq!(count, 32000);
        }
    }
    #[test]
    fn rejects_alias_frequency() {
        let rate = 48000;
        let input: Vec<f32> = (0..rate)
            .map(|i| (2. * std::f32::consts::PI * 12000. * i as f32 / rate as f32).sin())
            .collect();
        let y = Resampler::new(rate).push(&input);
        let rms = (y[100..]
            .iter()
            .map(|x| (*x as f64 / 32768.).powi(2))
            .sum::<f64>()
            / (y.len() - 100) as f64)
            .sqrt();
        assert!(rms < 0.01, "{rms}");
    }

    #[test]
    fn rate_converter_keeps_length_and_continuity_across_buffers() {
        let same = RateConverter::new(48_000, 48_000).push(vec![0.1, 0.2, 0.3]);
        assert_eq!(same, vec![0.1, 0.2, 0.3]);

        // A 44.1 kHz replacement device feeding a 48 kHz timeline.
        let mut convert = RateConverter::new(44_100, 48_000);
        let ramp: Vec<f32> = (0..44_100).map(|i| i as f32 / 44_100.0).collect();
        let mut out = Vec::new();
        for chunk in ramp.chunks(441) {
            out.extend(convert.push(chunk.to_vec()));
        }
        assert!((out.len() as i64 - 48_000).abs() <= 2, "{}", out.len());
        // A linear ramp stays a ramp: no jumps at buffer boundaries.
        let step = 44_100.0 / 48_000.0 / 44_100.0;
        for pair in out.windows(2) {
            assert!(((pair[1] - pair[0]) - step as f32).abs() < 1e-5);
        }

        let mut down = RateConverter::new(48_000, 16_000);
        let mut count = 0;
        for chunk in vec![0.5f32; 48_000].chunks(480) {
            let part = down.push(chunk.to_vec());
            assert!(part.iter().all(|v| (*v - 0.5).abs() < 1e-6));
            count += part.len();
        }
        assert!((count as i64 - 16_000).abs() <= 1);
    }
}
