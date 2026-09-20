#[cfg(not(windows))]
compile_error!("lectureedit-windows-loopback builds only for Windows targets");

use std::io::{self, Read, Write};
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use windows::core::{Result as WindowsResult, BOOL, GUID};
use windows::Win32::Foundation::TRUE;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Console::SetConsoleCtrlHandler;

const OUTPUT_RATE: f64 = 16_000.0;
const QUEUE_CAPACITY: usize = 32;
const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;
const PCM_SUBTYPE: GUID = GUID::from_values(
    0x00000001,
    0x0000,
    0x0010,
    [0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71],
);
const FLOAT_SUBTYPE: GUID = GUID::from_values(
    0x00000003,
    0x0000,
    0x0010,
    [0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71],
);

static CONSOLE_STOP: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn console_handler(_: u32) -> BOOL {
    CONSOLE_STOP.store(true, Ordering::Release);
    TRUE
}

#[repr(C)]
struct WaveFormatExtensible {
    format: WAVEFORMATEX,
    valid_bits_per_sample: u16,
    channel_mask: u32,
    sub_format: GUID,
}

#[derive(Clone, Copy)]
enum Encoding {
    Float,
    SignedInteger,
}

struct MixFormat {
    sample_rate: f64,
    channels: usize,
    block_align: usize,
    bytes_per_sample: usize,
    valid_bits: u16,
    encoding: Encoding,
}

impl MixFormat {
    unsafe fn from_wave_format(pointer: *const WAVEFORMATEX) -> Result<Self, String> {
        if pointer.is_null() {
            return Err("GetMixFormat returned a null pointer".into());
        }
        let format = &*pointer;
        let mut valid_bits = format.wBitsPerSample;
        let encoding = match format.wFormatTag {
            WAVE_FORMAT_PCM => Encoding::SignedInteger,
            WAVE_FORMAT_IEEE_FLOAT => Encoding::Float,
            WAVE_FORMAT_EXTENSIBLE if usize::from(format.cbSize) >= 22 => {
                let extended = &*(pointer.cast::<WaveFormatExtensible>());
                valid_bits = extended.valid_bits_per_sample;
                if extended.sub_format == PCM_SUBTYPE {
                    Encoding::SignedInteger
                } else if extended.sub_format == FLOAT_SUBTYPE {
                    Encoding::Float
                } else {
                    return Err(format!(
                        "unsupported extensible subtype {:?}",
                        extended.sub_format
                    ));
                }
            }
            tag => return Err(format!("unsupported wave format tag {tag:#06x}")),
        };
        let channels = usize::from(format.nChannels);
        let block_align = usize::from(format.nBlockAlign);
        if channels == 0 || block_align == 0 || format.nSamplesPerSec == 0 {
            return Err("invalid endpoint mix format".into());
        }
        let bytes_per_sample = block_align / channels;
        if bytes_per_sample == 0 || bytes_per_sample * channels != block_align {
            return Err("unsupported endpoint block alignment".into());
        }
        Ok(Self {
            sample_rate: f64::from(format.nSamplesPerSec),
            channels,
            block_align,
            bytes_per_sample,
            valid_bits,
            encoding,
        })
    }

    fn decode_mono(&self, bytes: &[u8], frames: usize, silent: bool) -> Result<Vec<f32>, String> {
        if silent {
            return Ok(vec![0.0; frames]);
        }
        let required = frames
            .checked_mul(self.block_align)
            .ok_or_else(|| "packet size overflow".to_string())?;
        if bytes.len() < required {
            return Err("WASAPI packet is shorter than its frame count".into());
        }
        let mut mono = Vec::with_capacity(frames);
        for frame in 0..frames {
            let mut sum = 0.0f32;
            for channel in 0..self.channels {
                let offset = frame * self.block_align + channel * self.bytes_per_sample;
                let sample = &bytes[offset..offset + self.bytes_per_sample];
                let value = match (self.encoding, self.bytes_per_sample) {
                    (Encoding::Float, 4) => f32::from_le_bytes(sample.try_into().unwrap()),
                    (Encoding::Float, 8) => f64::from_le_bytes(sample.try_into().unwrap()) as f32,
                    (Encoding::SignedInteger, 2) => {
                        f32::from(i16::from_le_bytes(sample.try_into().unwrap())) / 32_768.0
                    }
                    (Encoding::SignedInteger, 3) => {
                        let raw = i32::from_le_bytes([
                            sample[0],
                            sample[1],
                            sample[2],
                            if sample[2] & 0x80 != 0 { 0xff } else { 0 },
                        ]);
                        raw as f32 / 8_388_608.0
                    }
                    (Encoding::SignedInteger, 4) => {
                        let raw = i32::from_le_bytes(sample.try_into().unwrap());
                        raw as f32 / 2_147_483_648.0
                    }
                    _ => {
                        return Err(format!(
                            "unsupported sample representation: {} bytes, {} valid bits",
                            self.bytes_per_sample, self.valid_bits
                        ))
                    }
                };
                sum += value;
            }
            mono.push(sum / self.channels as f32);
        }
        Ok(mono)
    }
}

struct LinearResampler {
    source_rate: f64,
    source_base: i64,
    next_position: f64,
    previous: Option<f32>,
}

impl LinearResampler {
    fn new(source_rate: f64) -> Self {
        Self {
            source_rate,
            source_base: 0,
            next_position: 0.0,
            previous: None,
        }
    }

    fn process(&mut self, input: &[f32]) -> Vec<i16> {
        if input.is_empty() {
            return Vec::new();
        }
        let first = self.source_base;
        let last = first + input.len() as i64 - 1;
        let step = self.source_rate / OUTPUT_RATE;
        let mut output = Vec::with_capacity((input.len() as f64 / step).ceil() as usize + 1);
        while self.next_position.ceil() as i64 <= last {
            let low = self.next_position.floor() as i64;
            let high = self.next_position.ceil() as i64;
            if low < first - 1 {
                self.next_position += step;
                continue;
            }
            let low_value = if low == first - 1 {
                self.previous.unwrap_or(input[0])
            } else {
                input[(low - first) as usize]
            };
            let high_value = input[(high - first) as usize];
            let fraction = (self.next_position - low as f64) as f32;
            let value = low_value + (high_value - low_value) * fraction;
            output.push((value.clamp(-1.0, 1.0) * 32_767.0).round() as i16);
            self.next_position += step;
        }
        self.previous = input.last().copied();
        self.source_base += input.len() as i64;
        output
    }
}

struct Chunk {
    start: u64,
    samples: Vec<i16>,
}

fn write_samples(output: &mut impl Write, samples: &[i16]) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    output.write_all(&bytes)
}

fn write_gap(output: &mut impl Write, start: u64, end: u64) -> io::Result<()> {
    if end <= start {
        return Ok(());
    }
    let zeros = [0i16; 4096];
    let mut remaining = end - start;
    while remaining > 0 {
        let count = remaining.min(zeros.len() as u64) as usize;
        write_samples(output, &zeros[..count])?;
        remaining -= count as u64;
    }
    eprintln!("GAP:{start}:{end}");
    Ok(())
}

fn spawn_writer(
    produced: Arc<AtomicU64>,
) -> (SyncSender<Chunk>, thread::JoinHandle<io::Result<()>>) {
    let (sender, receiver) = sync_channel::<Chunk>(QUEUE_CAPACITY);
    let handle = thread::spawn(move || {
        let stdout = io::stdout();
        let mut output = stdout.lock();
        let mut cursor = 0u64;
        while let Ok(mut chunk) = receiver.recv() {
            if chunk.start > cursor {
                write_gap(&mut output, cursor, chunk.start)?;
                cursor = chunk.start;
            }
            if chunk.start < cursor {
                let overlap = (cursor - chunk.start) as usize;
                if overlap >= chunk.samples.len() {
                    continue;
                }
                chunk.samples.drain(..overlap);
            }
            write_samples(&mut output, &chunk.samples)?;
            cursor += chunk.samples.len() as u64;
        }
        let final_cursor = produced.load(Ordering::Acquire);
        if final_cursor > cursor {
            write_gap(&mut output, cursor, final_cursor)?;
        }
        output.flush()
    });
    (sender, handle)
}

fn send_bounded(sender: &SyncSender<Chunk>, samples: Vec<i16>, produced: &AtomicU64) -> bool {
    if samples.is_empty() {
        return true;
    }
    let count = samples.len() as u64;
    let start = produced.fetch_add(count, Ordering::AcqRel);
    match sender.try_send(Chunk { start, samples }) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => true,
        Err(TrySendError::Disconnected(_)) => false,
    }
}

unsafe fn run() -> Result<(), String> {
    CoInitializeEx(None, COINIT_MULTITHREADED)
        .ok()
        .map_err(|error| error.to_string())?;
    struct ComGuard;
    impl Drop for ComGuard {
        fn drop(&mut self) {
            unsafe { CoUninitialize() }
        }
    }
    let _com = ComGuard;

    // The app launches this pipe-based helper with CREATE_NO_WINDOW. Console control
    // delivery is optional; stdin newline/EOF remains the parent shutdown protocol.
    let _ = SetConsoleCtrlHandler(Some(console_handler), true);
    let stop = Arc::new(AtomicBool::new(false));
    let input_stop = Arc::clone(&stop);
    thread::spawn(move || {
        let mut byte = [0u8; 1];
        loop {
            match io::stdin().read(&mut byte) {
                Ok(1) if byte[0] != b'\n' => continue,
                Ok(_) | Err(_) => {
                    input_stop.store(true, Ordering::Release);
                    break;
                }
            }
        }
    });

    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
        .map_err(|error| error.to_string())?;
    let device = enumerator
        .GetDefaultAudioEndpoint(eRender, eConsole)
        .map_err(|error| error.to_string())?;
    let audio_client: IAudioClient = device
        .Activate(CLSCTX_ALL, None)
        .map_err(|error| error.to_string())?;
    let wave_pointer = audio_client
        .GetMixFormat()
        .map_err(|error| error.to_string())?;
    struct FormatGuard(*mut WAVEFORMATEX);
    impl Drop for FormatGuard {
        fn drop(&mut self) {
            unsafe { CoTaskMemFree(Some(self.0.cast())) }
        }
    }
    let wave_guard = FormatGuard(wave_pointer);
    let mix = MixFormat::from_wave_format(wave_guard.0)?;

    audio_client
        .Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            0,
            0,
            wave_guard.0,
            None,
        )
        .map_err(|error| error.to_string())?;
    let capture: IAudioCaptureClient = audio_client
        .GetService()
        .map_err(|error| error.to_string())?;
    let produced = Arc::new(AtomicU64::new(0));
    let (sender, writer) = spawn_writer(Arc::clone(&produced));
    let mut resampler = LinearResampler::new(mix.sample_rate);

    audio_client.Start().map_err(|error| error.to_string())?;
    eprintln!("READY");
    let capture_result: Result<(), String> = (|| {
        while !stop.load(Ordering::Acquire) && !CONSOLE_STOP.load(Ordering::Acquire) {
            let mut packet_frames = capture
                .GetNextPacketSize()
                .map_err(|error| error.to_string())?;
            if packet_frames == 0 {
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            while packet_frames > 0 {
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = Default::default();
                capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .map_err(|error| error.to_string())?;
                let byte_count = frames as usize * mix.block_align;
                let bytes = if data.is_null() {
                    &[]
                } else {
                    slice::from_raw_parts(data, byte_count)
                };
                let silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
                let decoded = mix.decode_mono(bytes, frames as usize, silent);
                capture
                    .ReleaseBuffer(frames)
                    .map_err(|error| error.to_string())?;
                let output = resampler.process(&decoded?);
                if !send_bounded(&sender, output, &produced) {
                    return Ok(());
                }
                packet_frames = capture
                    .GetNextPacketSize()
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    })();
    let stop_result: WindowsResult<()> = audio_client.Stop();
    drop(sender);
    let writer_result = writer
        .join()
        .map_err(|_| "stdout writer panicked".to_string())?;
    writer_result.map_err(|error| format!("stdout: {error}"))?;
    stop_result.map_err(|error| error.to_string())?;
    capture_result?;
    eprintln!("STOPPED");
    Ok(())
}

fn main() {
    if std::env::args_os().len() != 1 {
        eprintln!("ERROR:USAGE:This helper accepts no arguments");
        std::process::exit(64);
    }
    if let Err(error) = unsafe { run() } {
        eprintln!("ERROR:WASAPI:{}", error.replace('\n', " "));
        std::process::exit(1);
    }
}
