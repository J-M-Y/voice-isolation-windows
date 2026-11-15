//! Background controller connecting the UI to audio and model workers.
use std::{path::{Path, PathBuf}, sync::mpsc::{self, Receiver, Sender}, thread::{self, JoinHandle}, time::{Duration, Instant}};
use crate::{audio, processor::Processor, ui::{Command, Event}};

enum Finished {
    Recorded(Result<PathBuf, String>),
    Started(Result<(audio::AudioEngine, Option<String>), String>),
    Compared(Result<(Vec<f32>, Vec<f32>), String>),
    Played(Result<(), String>),
    Panicked,
}

fn spawn_job(tx: Sender<Finished>, job: impl FnOnce() -> Finished + Send + 'static) {
    thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).unwrap_or(Finished::Panicked);
        let _ = tx.send(result);
    });
}

fn status(tx: &Sender<Event>, message: impl Into<String>, busy: bool, running: bool) {
    let _ = tx.send(Event::Status { message: message.into(), busy, running });
}

fn model_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("VOICE_ISOLATION_MODELS") { return path.into(); }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let path = dir.join("models");
            if path.is_dir() { return path; }
            // A Cargo build can also use the repository's models directory.
            let project_models = dir.join("..").join("..").join("..").join("models");
            if project_models.is_dir() { return project_models; }
        }
    }
    PathBuf::from("models")
}

fn read_reference(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let mut wav = hound::WavReader::open(path).map_err(|e| format!("无法读取声样 {}：{e}", path.display()))?;
    let spec = wav.spec();
    if spec.channels == 0 || spec.sample_rate == 0 { return Err("声样的声道数或采样率无效".into()); }
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float if spec.bits_per_sample == 32 => wav.samples::<f32>().collect::<Result<_, _>>().map_err(|e| e.to_string())?,
        hound::SampleFormat::Int if spec.bits_per_sample <= 32 => {
            let scale = (2f64).powi(spec.bits_per_sample as i32 - 1) as f32;
            wav.samples::<i32>().map(|s| s.map(|v| v as f32 / scale)).collect::<Result<_, _>>().map_err(|e| e.to_string())?
        }
        _ => return Err("不支持这种 WAV 格式，请录制新的声样".into()),
    };
    let channels = spec.channels as usize;
    let mono: Vec<f32> = samples.chunks_exact(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32).collect();
    if mono.len() < spec.sample_rate as usize * 3 { return Err("声样太短，请至少录制 3 秒".into()); }
    let rms = (mono.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / mono.len() as f64).sqrt();
    if !rms.is_finite() || rms < 0.001 { return Err("声样音量过低或无效，请重新录制".into()); }
    Ok((mono, spec.sample_rate))
}

fn make_processor(reference: &Path, strong: bool, gpu: bool, block_size: usize, gpu_adapter: Option<(u32, u64)>) -> Result<Processor, String> {
    let (samples, rate) = read_reference(reference)?;
    if gpu {
        let reference16k = audio::gpu_reference(&samples, rate)?;
        Processor::new_gpu(&model_dir(), &reference16k, block_size / 3, strong, gpu_adapter)
    } else {
        Processor::new(&model_dir(), &samples, rate, strong)
    }
}

pub fn start() -> (Sender<Command>, Receiver<Event>, JoinHandle<()>) {
    let (command_tx, command_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let (done_tx, done_rx) = mpsc::channel();
        let mut engine: Option<audio::AudioEngine> = None;
        let mut current_gpu: Option<String> = None;
        let mut comparison: Option<(Vec<f32>, Vec<f32>)> = None;
        let mut last_stats = audio::AudioStats::default();
        let mut last_stats_update = Instant::now();
        loop {
            while let Ok(done) = done_rx.try_recv() {
                match done {
                    Finished::Recorded(result) => match result {
                        Ok(path) => { let _ = event_tx.send(Event::ReferenceRecorded(path)); },
                        Err(e) => status(&event_tx, format!("录制失败：{e}"), false, false),
                    },
                    Finished::Started(result) => match result {
                        Ok((value, device)) => {
                            last_stats = audio::AudioStats { output_sample_rate: value.stats().output_sample_rate, ..Default::default() };
                            engine = Some(value); last_stats_update = Instant::now();
                            current_gpu = device;
                            let message = match &current_gpu { Some(name) => format!("正在使用 {name} 加速声纹提取；降噪使用 CPU。"), None => "正在使用 CPU 处理音频。".into() };
                            status(&event_tx, message, false, true);
                        },
                        Err(e) => status(&event_tx, format!("启动失败：{e}"), false, false),
                    },
                    Finished::Compared(result) => match result {
                        Ok(value) => { comparison = Some(value); let _ = event_tx.send(Event::ComparisonReady); },
                        Err(e) => status(&event_tx, format!("试听失败：{e}"), false, false),
                    },
                    Finished::Played(result) => match result {
                        Ok(()) => status(&event_tx, "播放完成。", false, false),
                        Err(e) => status(&event_tx, format!("播放失败：{e}"), false, false),
                    },
                    Finished::Panicked => status(&event_tx, "后台任务异常退出，请重试。", false, false),
                }
            }
            if let Some(error) = engine.as_ref().and_then(|e| e.error()) {
                if let Some(mut old) = engine.take() { old.stop(); }
                current_gpu = None;
                status(&event_tx, format!("音频处理已停止：{error}"), false, false);
            }
            // Rate-limit UI updates; counters themselves are incremented only
            // on a dropped block or a callback that needed silence.
            if last_stats_update.elapsed() >= Duration::from_secs(1) {
                if let Some(current) = engine.as_ref().map(|e| e.stats()) {
                    if current != last_stats {
                        let prefix = current_gpu.as_ref().map(|name| format!("当前显卡：{name}。 ")).unwrap_or_default();
                        let missing_ms = if current.output_sample_rate == 0 { 0.0 } else { current.output_starved_frames as f64 * 1000.0 / current.output_sample_rate as f64 };
                        status(&event_tx, format!("{prefix}音频缓冲异常：输入丢块 {}，输出丢块 {}，输出缺样 {} 次（累计约 {:.1} 毫秒，含恢复缓冲）。若持续增加，请减轻系统负载。", current.input_drops, current.output_drops, current.output_starved, missing_ms), false, true);
                        last_stats = current;
                    }
                }
                last_stats_update = Instant::now();
            }
            match command_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Command::RefreshDevices) => match (audio::list_input_devices(), audio::list_output_devices()) {
                    (Ok(inputs), Ok(outputs)) => {
                        let default_output = audio::default_output_name();
                        let (gpu_adapters, gpu_error) = match crate::processor::list_gpu_adapters() {
                            Ok(adapters) => (adapters, None),
                            Err(error) => (Vec::new(), Some(error)),
                        };
                        let _ = event_tx.send(Event::Devices { inputs, outputs, default_output, gpu_adapters, gpu_error });
                    },
                    (Err(e), _) | (_, Err(e)) => status(&event_tx, format!("设备读取失败：{e}"), false, false),
                },
                Ok(Command::Record { input, input_name, path }) => { spawn_job(done_tx.clone(), move || { let result = audio::record_reference(input, &input_name, &path).map(|()| path); Finished::Recorded(result) }); },
                Ok(Command::Start { input, input_name, output, output_name, reference, strong_music, block_size, use_gpu, gpu_adapter }) => {
                    spawn_job(done_tx.clone(), move || {
                        let result = make_processor(&reference, strong_music, use_gpu, block_size, gpu_adapter).and_then(|mut processor| {
                            let device = processor.gpu_device_description();
                            audio::AudioEngine::start(input, input_name, output, output_name, block_size, move |samples| processor.process(samples)).map(|engine| (engine, device))
                        });
                        Finished::Started(result)
                    });
                },
                Ok(Command::Stop) => { if let Some(mut old) = engine.take() { old.stop(); } current_gpu = None; status(&event_tx, "处理已停止。", false, false); },
                Ok(Command::RecordComparison { input, input_name, reference, strong_music, block_size, use_gpu, gpu_adapter }) => {
                    comparison = None;
                    spawn_job(done_tx.clone(), move || {
                        let result = (|| {
                            if !audio::valid_processing_block_size(block_size) { return Err("无效的处理延迟设置".into()); }
                            let mut processor = make_processor(&reference, strong_music, use_gpu, block_size, gpu_adapter)?;
                            let original = audio::record_samples(input, &input_name, 5)?;
                            let mut processed = Vec::with_capacity(original.len());
                            for chunk in original.chunks(block_size) {
                                let mut block = vec![0.0; block_size];
                                block[..chunk.len()].copy_from_slice(chunk);
                                processed.extend(processor.process(&block)?);
                            }
                            processed.truncate(original.len());
                            Ok((original, processed))
                        })();
                        Finished::Compared(result)
                    });
                },
                Ok(Command::PlayComparison { output, output_name, processed }) => {
                    if let Some((raw, clean)) = &comparison {
                        let data = if processed { clean.clone() } else { raw.clone() };
                        spawn_job(done_tx.clone(), move || Finished::Played(audio::play_samples(output, &output_name, data)));
                    } else { status(&event_tx, "请先录制试听。", false, false); }
                },
                Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => (),
            }
        }
        if let Some(mut old) = engine { old.stop(); }
    });
    (command_tx, event_rx, handle)
}
