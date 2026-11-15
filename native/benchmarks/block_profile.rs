//! Offline, single-process model timing for each requested chunk size.
//! Copied into a temporary Cargo project beside the real processor.rs.
#[path = "processor.rs"]
mod processor;
use processor::Processor;
use std::{env, path::Path, time::Instant};
const RATE: u32 = 48_000;
const WARM: usize = 115_200; // 2.4 s, divisible by all tested chunk sizes.
const MEASURED: usize = 576_000; // 12 s.

#[repr(C)]
struct FileTime { low: u32, high: u32 }
#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> *mut std::ffi::c_void;
    fn GetProcessTimes(process: *mut std::ffi::c_void, creation: *mut FileTime,
        exit: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
}
fn cpu_seconds() -> Result<f64, String> {
    let mut create = FileTime { low: 0, high: 0 };
    let mut exit = FileTime { low: 0, high: 0 };
    let mut kernel = FileTime { low: 0, high: 0 };
    let mut user = FileTime { low: 0, high: 0 };
    if unsafe { GetProcessTimes(GetCurrentProcess(), &mut create, &mut exit, &mut kernel, &mut user) } == 0 {
        return Err("GetProcessTimes failed".into());
    }
    let secs = |t: FileTime| (((t.high as u64) << 32) | t.low as u64) as f64 * 1e-7;
    Ok(secs(kernel) + secs(user))
}
fn read(path: &Path) -> Result<Vec<f32>, String> {
    let mut w = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let s = w.spec();
    if s.sample_rate != RATE || s.channels != 1 || s.bits_per_sample != 16 || s.sample_format != hound::SampleFormat::Int {
        return Err("WAV must be mono 16-bit 48000 Hz".into());
    }
    w.samples::<i16>().map(|x| x.map(|x| x as f32 / 32768.0)).collect::<Result<Vec<_>,_>>().map_err(|e| e.to_string())
}
fn run() -> Result<(), String> {
    let args: Vec<_> = env::args_os().collect();
    if args.len() != 8 { return Err("usage: block_profile reference.wav input.wav model_dir block_samples normal|strong output.wav trial".into()); }
    let reference = read(Path::new(&args[1]))?;
    let input = read(Path::new(&args[2]))?;
    let block: usize = args[4].to_str().ok_or("block")?.parse().map_err(|_| "block")?;
    let strong = match args[5].to_str() { Some("normal") => false, Some("strong") => true, _ => return Err("invalid mode".into()) };
    let trial: usize = args[7].to_str().ok_or("trial")?.parse().map_err(|_| "trial")?;
    if ![3840, 7680, 11520].contains(&block) || input.len() < WARM + MEASURED { return Err("invalid block or input shorter than 14.4 seconds".into()); }
    let mut p = Processor::new(Path::new(&args[3]), &reference, RATE, strong)?;
    for b in input[..WARM].chunks_exact(block) { p.process(b)?; }
    let mut out = Vec::with_capacity(MEASURED);
    let cpu0 = cpu_seconds()?;
    let t = Instant::now();
    for b in input[WARM..WARM+MEASURED].chunks_exact(block) { out.extend(p.process(b)?); }
    let wall = t.elapsed().as_secs_f64();
    let cpu = cpu_seconds()? - cpu0;
    if out.len() != MEASURED || out.iter().any(|s| !s.is_finite()) { return Err("invalid model output".into()); }
    let rms = (out.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / out.len() as f64).sqrt();
    let spec = hound::WavSpec { channels: 1, sample_rate: RATE, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create(Path::new(&args[6]), spec).map_err(|e| e.to_string())?;
    for s in out { w.write_sample(s).map_err(|e| e.to_string())?; }
    w.finalize().map_err(|e| e.to_string())?;
    println!("{{\"mode\":\"{}\",\"block_samples\":{},\"trial\":{},\"audio_seconds\":12,\"cpu_seconds\":{:.6},\"wall_seconds\":{:.6},\"cpu_one_core_percent\":{:.4},\"rms\":{:.9}}}", if strong { "strong" } else { "normal" }, block, trial, cpu, wall, cpu / 12.0 * 100.0, rms);
    Ok(())
}
fn main() { if let Err(e) = run() { eprintln!("{e}"); std::process::exit(1); } }
