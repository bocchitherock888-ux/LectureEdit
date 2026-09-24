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
    pub captured: Arc<AtomicU64>,
    pub rate: u32,
    pub handle: CaptureHandle,
}
pub fn microphone() -> Result<Source, String> {
    let (tx, rx) = bounded(FRAME_QUEUE);
    let (etx, erx) = bounded(16);
    let (_gtx, grx) = bounded(1);
    let (ready, rdy) = bounded(1);
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let captured = Arc::new(AtomicU64::new(0));
    let callback_cursor = captured.clone();
    let thread = thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let host = cpal::default_host();
            let device = host
                .default_input_device()
                .ok_or("未找到麦克风，请连接输入设备")?;
            let config = device
                .default_input_config()
                .map_err(|e| format!("无法打开麦克风：{e}"))?;
            let rate = config.sample_rate().0;
            let channels = config.channels() as usize;
            let errors = etx.clone();
            let stream = match config.sample_format() {
                cpal::SampleFormat::F32 => build::<f32>(
                    &device,
                    &config.into(),
                    channels,
                    tx,
                    errors,
                    callback_cursor,
                ),
                cpal::SampleFormat::I16 => build::<i16>(
                    &device,
                    &config.into(),
                    channels,
                    tx,
                    errors,
                    callback_cursor,
                ),
                cpal::SampleFormat::U16 => build::<u16>(
                    &device,
                    &config.into(),
                    channels,
                    tx,
                    errors,
                    callback_cursor,
                ),
                f => return Err(format!("麦克风采样格式暂未支持：{f:?}")),
            }?;
            stream.play().map_err(|e| format!("麦克风启动失败：{e}"))?;
            let _ = ready.send(Ok(rate));
            while !flag.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(30));
            }
            drop(stream);
            Ok(())
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
                _ => "麦克风未就绪，请在系统设置中允许 LectureEdit 使用麦克风后重试".into(),
            })
        }
    }
}
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    tx: Sender<Frame>,
    errors: Sender<String>,
    captured: Arc<AtomicU64>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let mut cursor = 0u64;
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                let start = cursor;
                let samples: Vec<f32> = data
                    .chunks_exact(channels)
                    .map(|f| {
                        f.iter()
                            .map(|s| <f32 as cpal::FromSample<T>>::from_sample_(*s))
                            .sum::<f32>()
                            / channels as f32
                    })
                    .collect();
                cursor += samples.len() as u64;
                captured.store(cursor, Ordering::Release);
                // A full queue loses this buffer only; absolute source offsets reveal the gap.
                let _ = tx.try_send(Frame { start, samples });
            },
            move |e| {
                let _ = errors.try_send(format!("音频设备中断：{e}"));
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
        return "请在“系统设置”→“隐私与安全性”→“录屏与系统录音”中允许 LectureEdit，然后重新打开应用".into();
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
                _ => "请在系统设置中允许 LectureEdit 录制系统声音后重试".into(),
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
}
