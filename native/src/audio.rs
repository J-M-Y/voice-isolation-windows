//! Windows audio transport. Audio callbacks never run the neural models.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use crossbeam_channel::{Receiver, Sender, TrySendError};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const SAMPLE_RATE: u32 = 48_000;
pub const BLOCK_SIZE: usize = 3_840;
pub const LOW_CPU_BLOCK_SIZE: usize = BLOCK_SIZE * 2;
pub const LOWEST_CPU_BLOCK_SIZE: usize = BLOCK_SIZE * 3;

pub fn valid_processing_block_size(size: usize) -> bool {
    matches!(size, BLOCK_SIZE | LOW_CPU_BLOCK_SIZE | LOWEST_CPU_BLOCK_SIZE)
}

fn processing_queue_blocks(block_size: usize) -> usize {
    (BLOCK_SIZE * 4 / block_size).max(2)
}

fn devices(input: bool) -> Result<Vec<cpal::Device>, String> {
    let host = cpal::default_host();
    let items = if input { host.input_devices() } else { host.output_devices() }
        .map_err(|e| format!("无法枚举音频设备: {e}"))?;
    Ok(items.collect())
}

pub fn list_input_devices() -> Result<Vec<String>, String> {
    Ok(devices(true)?.iter().map(|d| d.name().unwrap_or_else(|_| "未知设备".into())).collect())
}

pub fn list_output_devices() -> Result<Vec<String>, String> {
    Ok(devices(false)?.iter().map(|d| d.name().unwrap_or_else(|_| "未知设备".into())).collect())
}

/// The system default playback endpoint, if Windows exposes one.
pub fn default_output_name() -> Option<String> {
    cpal::default_host().default_output_device().and_then(|d| d.name().ok())
}

fn device(index: usize, expected_name: &str, input: bool) -> Result<cpal::Device, String> {
    let items = devices(input)?;
    let matches = items.iter().filter(|d| d.name().ok().as_deref() == Some(expected_name)).count();
    if matches != 1 { return Err("设备已变更或存在同名设备，请刷新列表并排除同名设备".into()); }
    let selected = items.into_iter().nth(index).ok_or_else(|| "音频设备已不存在，请刷新设备列表".to_string())?;
    if selected.name().ok().as_deref() != Some(expected_name) { return Err("设备顺序已变更，请刷新设备列表并重新选择".into()); }
    Ok(selected)
}

// Windowed-sinc resampler with a 32-sample lookahead. The kernel is computed
// only at startup. Downsampling applies a low-pass to prevent aliasing.
struct Resampler {
    input: VecDeque<f32>, position: f64, step: f64, kernels: Vec<[f32; 33]>, identity: bool,
}
impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        let identity = from == to;
        let mut kernels = Vec::new();
        if !identity {
            let cutoff = (to as f64 / from as f64).min(1.0) * 0.94;
            for phase in 0..1024 {
                let frac = phase as f64 / 1024.0;
                let mut kernel = [0.0; 33];
                let mut sum = 0.0;
                for (i, k) in kernel.iter_mut().enumerate() {
                    let x = i as f64 - 16.0 - frac;
                    let sinc = if x.abs() < 1e-9 { cutoff } else { (std::f64::consts::PI * cutoff * x).sin() / (std::f64::consts::PI * x) };
                    let window = 0.5 + 0.5 * (std::f64::consts::PI * x / 17.0).cos();
                    *k = (sinc * window) as f32;
                    sum += *k as f64;
                }
                for k in &mut kernel { *k /= sum as f32; }
                kernels.push(kernel);
            }
        }
        let mut input = VecDeque::new();
        if !identity { input.extend(std::iter::repeat(0.0).take(16)); }
        Self { input, position: 16.0, step: from as f64 / to as f64, kernels, identity }
    }
    fn push(&mut self, sample: f32, output: &mut Vec<f32>) {
        if self.identity { output.push(sample); return; }
        self.input.push_back(sample);
        while (self.position.floor() as usize) + 16 < self.input.len() {
            let center = self.position.floor() as usize;
            let phase = (((self.position - center as f64) * 1024.0) as usize).min(1023);
            let mut value = 0.0;
            for (i, k) in self.kernels[phase].iter().enumerate() { value += self.input[center - 16 + i] * k; }
            output.push(value);
            self.position += self.step;
            let discard = (self.position.floor() as usize).saturating_sub(16);
            for _ in 0..discard { self.input.pop_front(); }
            self.position -= discard as f64;
        }
    }
}

/// Prepare the three-second enrollment expected by the isolated GPU worker.
/// Select the loudest continuous window within the first 30 seconds, using
/// 10 ms steps as in the CPU VoxLock pipeline. This is offline startup work;
/// live audio is never used to replace a missing or invalid reference.
pub fn gpu_reference(samples: &[f32], input_rate: u32) -> Result<Vec<f32>, String> {
    const RATE: u32 = 16_000;
    const FRAME: usize = 160;
    const WINDOW: usize = 3 * RATE as usize;
    if input_rate == 0 || samples.len() < input_rate as usize * 3 || !samples.iter().all(|v| v.is_finite()) {
        return Err("声样至少需要 3 秒有效音频".into());
    }
    let limit = samples.len().min((input_rate as usize).saturating_mul(30));
    let source = &samples[..limit];
    let expected = ((source.len() as u64 * RATE as u64) / input_rate as u64) as usize;
    let mut converted = Vec::with_capacity(expected + 64);
    let mut resampler = Resampler::new(input_rate, RATE);
    // Match the CPU path's temporary 16-bit WAV before resampling.
    for &sample in source {
        let pcm = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
        resampler.push(pcm as f32 / 32768.0, &mut converted);
    }
    if input_rate != RATE {
        // The windowed filter needs future samples to finish its last outputs.
        for _ in 0..32 { resampler.push(0.0, &mut converted); }
    }
    converted.truncate(expected);
    if converted.len() < WINDOW { return Err("声样重采样后不足 3 秒，请重新录制".into()); }
    let mut energies = Vec::with_capacity(converted.len() / FRAME);
    for frame in converted.chunks_exact(FRAME) {
        energies.push(frame.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>());
    }
    let frames_in_window = WINDOW / FRAME;
    let mut sum: f64 = energies[..frames_in_window].iter().sum();
    let mut best = sum;
    let mut best_frame = 0;
    for end in frames_in_window..energies.len() {
        sum += energies[end] - energies[end - frames_in_window];
        if sum > best { best = sum; best_frame = end + 1 - frames_in_window; }
    }
    if !best.is_finite() || best / WINDOW as f64 <= 1e-6 {
        return Err("声样音量过低或无效，请重新录制".into());
    }
    let start = best_frame * FRAME;
    Ok(converted[start..start + WINDOW].to_vec())
}

// Drop the oldest queued block when processing falls behind. This bounds both
// memory and latency; an overflowing input never bypasses the model.
fn send_latest<T>(tx: &Sender<T>, oldest: &Receiver<T>, value: T) -> bool {
    match tx.try_send(value) {
        Ok(()) => false,
        Err(TrySendError::Full(value)) => {
            let _ = oldest.try_recv();
            let _ = tx.try_send(value);
            true
        }
        Err(TrySendError::Disconnected(_)) => true,
    }
}

fn input_stream(device: &cpal::Device, tx: Sender<Vec<f32>>, oldest: Receiver<Vec<f32>>, overrun: Arc<AtomicU64>, error: Arc<Mutex<Option<String>>>, block_size: usize) -> Result<cpal::Stream, String> {
    let supported = device.default_input_config().map_err(|e| format!("无法读取输入格式: {e}"))?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let channels = config.channels as usize;
    let mut resampler = Resampler::new(config.sample_rate.0, SAMPLE_RATE);
    let mut block = Vec::with_capacity(block_size);
    let mut converted = Vec::with_capacity(4);
    let mut consume = move |sample: f32| {
        converted.clear();
        resampler.push(sample, &mut converted);
        for &v in &converted {
            block.push(v);
            if block.len() == block_size {
                let full = std::mem::replace(&mut block, Vec::with_capacity(block_size));
                if send_latest(&tx, &oldest, full) { overrun.fetch_add(1, Ordering::Relaxed); }
            }
        }
    };
    let err = move |e| { if let Ok(mut slot) = error.lock() { *slot = Some(format!("输入设备错误: {e}")); } };
    let stream = match format {
        SampleFormat::F32 => device.build_input_stream(&config, move |data: &[f32], _| { for frame in data.chunks_exact(channels) { consume(frame[0]); } }, err, None),
        SampleFormat::I16 => device.build_input_stream(&config, move |data: &[i16], _| { for frame in data.chunks_exact(channels) { consume(frame[0] as f32 / 32768.0); } }, err, None),
        SampleFormat::U16 => device.build_input_stream(&config, move |data: &[u16], _| { for frame in data.chunks_exact(channels) { consume((frame[0] as f32 - 32768.0) / 32768.0); } }, err, None),
        _ => return Err(format!("不支持的输入采样格式: {format:?}")),
    };
    stream.map_err(|e| format!("无法打开输入设备: {e}"))
}

// Return one underrun indication per callback, regardless of how many frames
// needed silence. This keeps the counter meaningful for large device buffers.
#[cfg(test)]
fn fill_output<T: Copy>(data: &mut [T], channels: usize, next: &mut impl FnMut() -> Option<f32>, convert: impl Fn(f32) -> T) -> bool {
    let mut missed = false;
    for frame in data.chunks_mut(channels) {
        // A newly queued block will be picked up in the next callback. Avoid
        // polling the channel for every sample after this callback runs dry.
        let sample = if missed { 0.0 } else { next().unwrap_or_else(|| { missed = true; 0.0 }) };
        frame.fill(convert(sample));
    }
    missed
}

// The playback reserve is deliberately smaller than a whole processing block.
// It is additional latency, not a capacity limit: output frames are held for
// this duration before playback starts, absorbing producer timing jitter.
fn playback_reserve_ms(block_size: usize) -> u64 {
    match block_size {
        BLOCK_SIZE => 20,
        LOW_CPU_BLOCK_SIZE => 40,
        LOWEST_CPU_BLOCK_SIZE => 80,
        _ => 0,
    }
}

struct PlaybackBuffer {
    rx: Receiver<Vec<f32>>,
    block: Vec<f32>,
    offset: usize,
    reserve_frames: usize,
    reserve_remaining: usize,
    waiting: bool,
    started: bool,
    startup_frames: usize,
    startup_grace_frames: usize,
}

impl PlaybackBuffer {
    fn new(rx: Receiver<Vec<f32>>, reserve_frames: usize, startup_grace_frames: usize) -> Self {
        Self { rx, block: Vec::new(), offset: 0, reserve_frames, reserve_remaining: 0, waiting: true, started: false, startup_frames: 0, startup_grace_frames }
    }

    // Returns the exact number of frames replaced by silence after playback
    // began. Startup prefill is intentional, and is not counted as an xrun.
    fn fill<T: Copy>(&mut self, data: &mut [T], channels: usize, convert: impl Fn(f32) -> T) -> usize {
        let mut missing = 0;
        let mut unavailable = false;
        for frame in data.chunks_mut(channels) {
            let suppress_initial = !self.started && self.startup_frames < self.startup_grace_frames;
            if !self.started { self.startup_frames = self.startup_frames.saturating_add(1); }
            let sample = loop {
                if unavailable {
                    if !suppress_initial { missing += 1; }
                    break 0.0;
                }
                if self.waiting {
                    match self.rx.try_recv() {
                        Ok(block) if !block.is_empty() => {
                            self.block = block;
                            self.offset = 0;
                            self.reserve_remaining = self.reserve_frames;
                            self.waiting = false;
                        }
                        _ => { unavailable = true; continue; }
                    }
                }
                if self.reserve_remaining != 0 {
                    self.reserve_remaining -= 1;
                    if !suppress_initial { missing += 1; }
                    break 0.0;
                }
                if self.offset < self.block.len() {
                    let value = self.block[self.offset];
                    self.offset += 1;
                    self.started = true;
                    break value.clamp(-1.0, 1.0);
                }
                match self.rx.try_recv() {
                    Ok(block) if !block.is_empty() => { self.block = block; self.offset = 0; }
                    _ => { self.waiting = true; unavailable = true; }
                }
            };
            frame.fill(convert(sample));
        }
        missing
    }
}

fn output_stream(device: &cpal::Device, rx: Receiver<Vec<f32>>, starved: Arc<AtomicU64>, starved_frames: Arc<AtomicU64>, reserve_ms: u64, error: Arc<Mutex<Option<String>>>) -> Result<(cpal::Stream, u32), String> {
    let supported = device.default_output_config().map_err(|e| format!("无法读取输出格式: {e}"))?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let rate = config.sample_rate.0;
    let channels = config.channels as usize;
    let reserve_frames = (u64::from(rate) * reserve_ms / 1000) as usize;
    let mut playback = PlaybackBuffer::new(rx, reserve_frames, rate as usize * 2);
    let err = move |e| { if let Ok(mut slot) = error.lock() { *slot = Some(format!("输出设备错误: {e}")); } };
    let stream = match format {
        SampleFormat::F32 => device.build_output_stream(&config, move |data: &mut [f32], _| { let n = playback.fill(data, channels, |s| s); if n != 0 { starved.fetch_add(1, Ordering::Relaxed); starved_frames.fetch_add(n as u64, Ordering::Relaxed); } }, err, None),
        SampleFormat::I16 => device.build_output_stream(&config, move |data: &mut [i16], _| { let n = playback.fill(data, channels, |s| (s * 32767.0) as i16); if n != 0 { starved.fetch_add(1, Ordering::Relaxed); starved_frames.fetch_add(n as u64, Ordering::Relaxed); } }, err, None),
        SampleFormat::U16 => device.build_output_stream(&config, move |data: &mut [u16], _| { let n = playback.fill(data, channels, |s| ((s * 32767.0) + 32768.0) as u16); if n != 0 { starved.fetch_add(1, Ordering::Relaxed); starved_frames.fetch_add(n as u64, Ordering::Relaxed); } }, err, None),
        _ => return Err(format!("不支持的输出采样格式: {format:?}")),
    }.map_err(|e| format!("无法打开输出设备: {e}"))?;
    Ok((stream, rate))
}

pub struct AudioEngine {
    stop: Arc<AtomicBool>, worker: Option<JoinHandle<()>>, error: Arc<Mutex<Option<String>>>,
    input_drops: Arc<AtomicU64>, output_drops: Arc<AtomicU64>, output_starved: Arc<AtomicU64>, output_starved_frames: Arc<AtomicU64>, output_sample_rate: u32,
}
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct AudioStats { pub input_drops: u64, pub output_drops: u64, pub output_starved: u64, pub output_starved_frames: u64, pub output_sample_rate: u32 }
impl AudioEngine {
    pub fn start<F>(input: usize, input_name: String, output: usize, output_name: String, block_size: usize, mut processor: F) -> Result<Self, String>
    where F: FnMut(&[f32]) -> Result<Vec<f32>, String> + Send + 'static {
        if !valid_processing_block_size(block_size) { return Err("无效的处理延迟设置".into()); }
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let input_drops = Arc::new(AtomicU64::new(0));
        let output_drops = Arc::new(AtomicU64::new(0));
        let output_starved = Arc::new(AtomicU64::new(0));
        let output_starved_frames = Arc::new(AtomicU64::new(0));
        let input_drops_thread = input_drops.clone();
        let output_drops_thread = output_drops.clone();
        let output_starved_thread = output_starved.clone();
        let output_starved_frames_thread = output_starved_frames.clone();
        let stop_thread = stop.clone();
        let error_thread = error.clone();
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let run = || -> Result<(), String> {
                let input_device = device(input, &input_name, true)?;
                let output_device = device(output, &output_name, false)?;
                // Keep the maximum queue duration bounded when larger blocks
                // are selected; two blocks are needed to absorb scheduling jitter.
                let queue_blocks = processing_queue_blocks(block_size);
                let (in_tx, in_rx) = crossbeam_channel::bounded(queue_blocks);
                let (out_tx, out_rx) = crossbeam_channel::bounded(queue_blocks);
                let input_stream = input_stream(&input_device, in_tx, in_rx.clone(), input_drops_thread, error_thread.clone(), block_size)?;
                let (output_stream, output_rate) = output_stream(&output_device, out_rx.clone(), output_starved_thread, output_starved_frames_thread, playback_reserve_ms(block_size), error_thread.clone())?;
                let mut resampler = Resampler::new(SAMPLE_RATE, output_rate);
                input_stream.play().map_err(|e| format!("无法启动输入: {e}"))?;
                output_stream.play().map_err(|e| format!("无法启动输出: {e}"))?;
                let _ = started_tx.send(Ok(output_rate));
                while !stop_thread.load(Ordering::Acquire) {
                    if let Some(e) = error_thread.lock().ok().and_then(|mut e| e.take()) { return Err(e); }
                    let block = match in_rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(b) => b, Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                        Err(_) => return Err("输入设备已断开".into()),
                    };
                    let processed = processor(&block)?;
                    if processed.len() != block_size || !processed.iter().all(|s| s.is_finite()) {
                        return Err("模型输出无效".into());
                    }
                    let mut converted = Vec::with_capacity((block_size as u64 * output_rate as u64 / SAMPLE_RATE as u64 + 4) as usize);
                    for sample in processed { resampler.push(sample.clamp(-1.0, 1.0), &mut converted); }
                    if send_latest(&out_tx, &out_rx, converted) { output_drops_thread.fetch_add(1, Ordering::Relaxed); }
                }
                Ok(())
            };
            if let Err(e) = run() {
                let _ = started_tx.try_send(Err(e.clone()));
                if let Ok(mut slot) = error_thread.lock() { *slot = Some(e); }
            }
        });
        match started_rx.recv() {
            Ok(Ok(output_sample_rate)) => Ok(Self { stop, worker: Some(worker), error, input_drops, output_drops, output_starved, output_starved_frames, output_sample_rate }),
            Ok(Err(e)) => { let _ = worker.join(); Err(e) },
            Err(_) => { let _ = worker.join(); Err("音频线程意外退出".into()) },
        }
    }
    pub fn error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|e| e.clone()).or_else(|| {
            if !self.stop.load(Ordering::Acquire) && self.worker.as_ref().is_some_and(|worker| worker.is_finished()) {
                Some("音频线程意外退出".into())
            } else { None }
        })
    }
    pub fn stats(&self) -> AudioStats {
        AudioStats { input_drops: self.input_drops.load(Ordering::Relaxed), output_drops: self.output_drops.load(Ordering::Relaxed), output_starved: self.output_starved.load(Ordering::Relaxed), output_starved_frames: self.output_starved_frames.load(Ordering::Relaxed), output_sample_rate: self.output_sample_rate }
    }
    pub fn stop(&mut self) { self.stop.store(true, Ordering::Release); if let Some(worker) = self.worker.take() { let _ = worker.join(); } }
}
impl Drop for AudioEngine { fn drop(&mut self) { self.stop(); } }

/// Records mono 48 kHz samples. Run on a worker thread.
pub fn record_samples(index: usize, expected_name: &str, seconds: u32) -> Result<Vec<f32>, String> {
    if seconds == 0 || seconds > 60 { return Err("录音时长必须在 1 到 60 秒之间".into()); }
    let input_device = device(index, expected_name, true)?;
    let (tx, rx) = crossbeam_channel::bounded(4);
    let error = Arc::new(Mutex::new(None));
    let overrun = Arc::new(AtomicU64::new(0));
    let stream = input_stream(&input_device, tx, rx.clone(), overrun.clone(), error.clone(), BLOCK_SIZE)?;
    stream.play().map_err(|e| format!("无法启动录音: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(seconds as u64 + 6);
    let target = SAMPLE_RATE as usize * seconds as usize;
    let mut samples = Vec::with_capacity(target);
    while samples.len() < target {
        if let Some(e) = error.lock().ok().and_then(|mut e| e.take()) { return Err(e); }
        if overrun.load(Ordering::Relaxed) != 0 { return Err("录音数据丢失，请重试并关闭占用过高的程序".into()); }
        if Instant::now() >= deadline { return Err("录音超时，请检查麦克风".into()); }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(block) => samples.extend(block), Err(crossbeam_channel::RecvTimeoutError::Timeout) => (),
            Err(_) => return Err("录音设备已断开".into()),
        }
    }
    drop(stream);
    if overrun.load(Ordering::Relaxed) != 0 { return Err("录音数据丢失，请重试并关闭占用过高的程序".into()); }
    samples.truncate(target);
    Ok(samples)
}

/// Records 12 seconds at 48 kHz to a 16-bit mono WAV. Run on a worker thread.
pub fn record_reference(index: usize, expected_name: &str, path: &Path) -> Result<(), String> {
    let samples = record_samples(index, expected_name, 12)?;
    let mean_square = samples.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / samples.len() as f64;
    if !mean_square.is_finite() || mean_square.sqrt() < 0.001 { return Err("录音音量过低或无效，请检查所选麦克风后重新录制".into()); }
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    // Keep an existing enrollment intact until recording and WAV finalization
    // both succeed. A same-directory rename then replaces it in one step.
    let mut temp = path.as_os_str().to_os_string();
    temp.push(format!(".{}.tmp", std::process::id()));
    let temp = std::path::PathBuf::from(temp);
    let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp).map_err(|e| format!("无法创建临时录音: {e}"))?;
    let write_result = (|| -> Result<(), String> {
        let spec = hound::WavSpec { channels: 1, sample_rate: SAMPLE_RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut wav = hound::WavWriter::new(file, spec).map_err(|e| e.to_string())?;
        for sample in samples { wav.write_sample((sample.clamp(-1.0, 1.0) * 32767.0) as i16).map_err(|e| e.to_string())?; }
        wav.finalize().map_err(|e| e.to_string())?;
        replace_file(&temp, path).map_err(|e| format!("无法保存录音: {e}"))
    })();
    if write_result.is_err() { let _ = std::fs::remove_file(&temp); }
    write_result
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" { fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32; }
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x1 | 0x8) } == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}
#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> { std::fs::rename(from, to) }

/// Play a finite mono 48 kHz recording, returning when the device has drained.
pub fn play_samples(index: usize, expected_name: &str, samples: Vec<f32>) -> Result<(), String> {
    if !samples.iter().all(|s| s.is_finite()) { return Err("音频数据无效".into()); }
    let device = device(index, expected_name, false)?;
    let error = Arc::new(Mutex::new(None));
    let (tx, rx) = crossbeam_channel::bounded(4);
    let (stream, rate) = output_stream(&device, rx, Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)), 0, error.clone())?;
    let mut resampler = Resampler::new(SAMPLE_RATE, rate);
    stream.play().map_err(|e| format!("无法启动播放: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs_f64(samples.len() as f64 / SAMPLE_RATE as f64 + 6.0);
    for chunk in samples.chunks(BLOCK_SIZE) {
        let mut converted = Vec::with_capacity((chunk.len() as u64 * rate as u64 / SAMPLE_RATE as u64 + 4) as usize);
        for &s in chunk { resampler.push(s.clamp(-1.0, 1.0), &mut converted); }
        loop {
            if let Some(e) = error.lock().ok().and_then(|mut e| e.take()) { return Err(e); }
            if Instant::now() >= deadline { return Err("播放超时，请检查输出设备".into()); }
            match tx.try_send(converted) {
                Ok(()) => break,
                Err(TrySendError::Full(b)) => { converted = b; thread::sleep(Duration::from_millis(10)); },
                Err(TrySendError::Disconnected(_)) => return Err("播放设备已断开".into()),
            }
        }
    }
    // At most four 80 ms blocks remain queued, plus the one being played.
    let drain_until = Instant::now() + Duration::from_millis(500);
    while Instant::now() < drain_until {
        if let Some(e) = error.lock().ok().and_then(|mut e| e.take()) { return Err(e); }
        thread::sleep(Duration::from_millis(10));
    }
    drop(stream);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn gpu_reference_selects_loudest_three_seconds() {
        let mut samples = vec![0.01; 5 * 16_000];
        samples[16_000..4 * 16_000].fill(0.5);
        let reference = gpu_reference(&samples, 16_000).unwrap();
        assert_eq!(reference.len(), 48_000);
        assert!(reference.iter().all(|s| *s > 0.49));
        let resampled = gpu_reference(&vec![0.25; 3 * 48_000], 48_000).unwrap();
        assert_eq!(resampled.len(), 48_000);
        assert!(gpu_reference(&vec![0.0; 3 * 16_000], 16_000).is_err());
    }
    #[test] fn processing_modes_keep_queues_bounded() {
        assert_eq!(processing_queue_blocks(BLOCK_SIZE), 4);
        for size in [BLOCK_SIZE, LOW_CPU_BLOCK_SIZE, LOWEST_CPU_BLOCK_SIZE] {
            assert!(valid_processing_block_size(size));
            assert!(processing_queue_blocks(size) * size <= SAMPLE_RATE as usize / 2);
        }
        assert!(!valid_processing_block_size(0));
        assert!(!valid_processing_block_size(BLOCK_SIZE + 1));
    }
    #[test] fn resampler_preserves_dc_and_rate() {
        for (from, to) in [(44_100, 48_000), (48_000, 44_100), (48_000, 48_000)] {
            let mut r = Resampler::new(from, to);
            let mut out = Vec::new();
            for _ in 0..from { r.push(0.5, &mut out); }
            assert!((out.len() as isize - to as isize).abs() < 100);
            assert!(out.iter().skip(100).all(|v| (v - 0.5).abs() < 0.001));
        }
    }
    #[test] fn queue_overflow_discards_oldest() {
        let (tx, rx) = crossbeam_channel::bounded(2);
        assert!(!send_latest(&tx, &rx, 1));
        assert!(!send_latest(&tx, &rx, 2));
        assert!(send_latest(&tx, &rx, 3));
        assert_eq!(rx.try_recv().unwrap(), 2);
        assert_eq!(rx.try_recv().unwrap(), 3);
    }
    #[test] fn output_underrun_silences_missing_frames_and_reports_once() {
        let mut data = [9.0f32; 8];
        let mut samples = [Some(0.25), Some(-0.5), None, Some(0.75)].into_iter();
        assert!(fill_output(&mut data, 2, &mut || samples.next().flatten(), |v| v));
        assert_eq!(data, [0.25, 0.25, -0.5, -0.5, 0.0, 0.0, 0.0, 0.0]);
        let mut data = [0.0f32; 4];
        assert!(!fill_output(&mut data, 2, &mut || Some(0.75), |v| v));
        assert_eq!(data, [0.75; 4]);
    }
    #[test] fn playback_prefill_and_recovery_count_missing_frames_exactly() {
        let (tx, rx) = crossbeam_channel::bounded(2);
        let mut playback = PlaybackBuffer::new(rx, 2, 100);
        let mut initial = [9.0; 4];
        assert_eq!(playback.fill(&mut initial, 2, |v| v), 0);
        assert_eq!(initial, [0.0; 4]);
        tx.send(vec![0.1, 0.2, 0.3, 0.4]).unwrap();
        let mut first = [9.0; 12];
        assert_eq!(playback.fill(&mut first, 2, |v| v), 0);
        assert_eq!(first, [0.0, 0.0, 0.0, 0.0, 0.1, 0.1, 0.2, 0.2, 0.3, 0.3, 0.4, 0.4]);
        let mut missing = [9.0; 6];
        assert_eq!(playback.fill(&mut missing, 2, |v| v), 3);
        assert_eq!(missing, [0.0; 6]);
        tx.send(vec![0.5, 0.6, 0.7, 0.8]).unwrap();
        let mut recovered = [9.0; 10];
        assert_eq!(playback.fill(&mut recovered, 2, |v| v), 2);
        assert_eq!(recovered, [0.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.6, 0.6, 0.7, 0.7]);
    }
    #[test] fn no_first_block_reports_missing_after_startup_grace() {
        let (_tx, rx) = crossbeam_channel::bounded(2);
        let mut playback = PlaybackBuffer::new(rx, 2, 4);
        let mut data = [1.0; 12];
        assert_eq!(playback.fill(&mut data, 2, |v| v), 2);
        assert_eq!(data, [0.0; 12]);
    }
    #[test] fn playback_reserve_absorbs_producer_jitter_without_hiding_underflow() {
        fn run(reserve_frames: usize) -> (usize, Option<usize>, Vec<f32>) {
            let (tx, rx) = crossbeam_channel::bounded(2);
            let mut playback = PlaybackBuffer::new(rx, reserve_frames, 2000);
            let mut missing = 0;
            let mut first_output = None;
            let mut output_samples = Vec::new();
            for t in (0..2900).step_by(10) {
                // All blocks finish in less than 240 ms. The second takes
                // 60 ms longer than the first, and arrives correspondingly late.
                for index in 0..11 {
                    if t == 360 + index * 240 + if index == 1 { 60 } else { 0 } {
                        tx.send(vec![(index + 1) as f32 / 20.0; 240]).unwrap();
                    }
                }
                let mut output = [0.0; 10];
                missing += playback.fill(&mut output, 1, |v| v);
                if first_output.is_none() {
                    first_output = output.iter().position(|v| *v != 0.0).map(|offset| t + offset);
                }
                output_samples.extend(output.iter().copied().filter(|v| *v != 0.0));
            }
            (missing, first_output, output_samples)
        }
        let baseline = run(0);
        let buffered = run(80);
        assert_eq!((baseline.0, baseline.1), (60, Some(360)));
        assert_eq!((buffered.0, buffered.1), (0, Some(440)));
        for (i, value) in buffered.2.iter().enumerate() {
            assert_eq!(*value, (i / 240 + 1) as f32 / 20.0);
        }
        assert_eq!((playback_reserve_ms(BLOCK_SIZE), playback_reserve_ms(LOW_CPU_BLOCK_SIZE), playback_reserve_ms(LOWEST_CPU_BLOCK_SIZE)), (20, 40, 80));
    }
    #[test] fn downsampling_suppresses_out_of_band_signal() {
        let mut r = Resampler::new(48_000, 16_000);
        let mut out = Vec::new();
        for n in 0..48_000 {
            r.push((2.0 * std::f32::consts::PI * 12_000.0 * n as f32 / 48_000.0).sin(), &mut out);
        }
        let steady = &out[100..];
        let rms = (steady.iter().map(|v| v * v).sum::<f32>() / steady.len() as f32).sqrt();
        assert!(rms < 0.05, "out-of-band RMS {rms}");
    }
}
