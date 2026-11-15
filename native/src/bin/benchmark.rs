//! Offline, device-free model smoke test and throughput benchmark.
#![allow(dead_code)]
#[path = "../processor.rs"]
mod processor;
use std::{env, path::{Path, PathBuf}, time::Instant};
use processor::Processor;

fn read_wav(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    if spec.channels == 0 || spec.sample_rate == 0 { return Err("无效 WAV 格式".into()); }
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_,_>>().map_err(|e| e.to_string())?,
        hound::SampleFormat::Int => {
            if spec.bits_per_sample == 0 || spec.bits_per_sample > 32 { return Err("不支持的 WAV 位深".into()); }
            let scale = 2.0f32.powi(spec.bits_per_sample as i32 - 1);
            reader.samples::<i32>().map(|x| x.map(|v| v as f32 / scale)).collect::<Result<_,_>>().map_err(|e| e.to_string())?
        }
    };
    let channels = spec.channels as usize;
    let mono = samples.chunks_exact(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32).collect();
    Ok((mono, spec.sample_rate))
}
fn run() -> Result<(), String> {
    let args: Vec<_> = env::args_os().collect();
    if args.len() != 5 { return Err("用法: benchmark <声样.wav> <48k输入.wav> <模型目录> <输出文件前缀>".into()); }
    let (reference, reference_rate) = read_wav(Path::new(&args[1]))?;
    let (input, rate) = read_wav(Path::new(&args[2]))?;
    if rate != 48000 || input.is_empty() { return Err("输入 WAV 必须为非空 48000 Hz 音频".into()); }
    for strong in [false, true] {
        let name = if strong { "dpdfnet2" } else { "gtcrn" };
        let mut processor = Processor::new(Path::new(&args[3]), &reference, reference_rate, strong)?;
        let start = Instant::now();
        let mut result = Vec::with_capacity(input.len() + 3840);
        for block in input.chunks(3840) {
            let mut padded = block.to_vec();
            padded.resize(3840, 0.0);
            result.extend(processor.process(&padded)?);
        }
        let elapsed = start.elapsed().as_secs_f64();
        result.truncate(input.len());
        if result.iter().any(|x| !x.is_finite()) { return Err(format!("{name} 返回非有限音频样本")); }
        if !result.iter().any(|x| x.abs() > 1e-8) { return Err(format!("{name} 输出全静音；请用包含人声的音频检查模型")); }
        let mut destination = PathBuf::from(&args[4]).into_os_string();
        destination.push(format!("-{name}.wav"));
        let destination = PathBuf::from(destination);
        let spec = hound::WavSpec { channels: 1, sample_rate: 48000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let mut writer = hound::WavWriter::create(&destination, spec).map_err(|e| e.to_string())?;
        for sample in result { writer.write_sample(sample).map_err(|e| e.to_string())?; }
        writer.finalize().map_err(|e| e.to_string())?;
        let duration = input.len() as f64 / 48000.0;
        println!("{name}: 音频 {duration:.2} 秒，用时 {elapsed:.2} 秒，实时系数 {:.3}，输出 {}", elapsed / duration, destination.display());
    }
    Ok(())
}
fn main() { if let Err(e) = run() { eprintln!("{e}"); std::process::exit(1); } }
