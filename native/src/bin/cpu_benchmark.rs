//! Repeatable offline CPU benchmark; no microphones or personal recordings.
#![allow(dead_code)]
#[path = "../processor.rs"]
mod processor;
use processor::Processor;
use std::{env, path::Path, time::Instant};
const RATE: u32 = 48_000;
const BLOCK: usize = 3_840;

#[cfg(windows)]
fn process_cpu_seconds() -> Result<f64, String> {
    use std::ffi::c_void;
    #[repr(C)] struct FileTime { low: u32, high: u32 }
    #[link(name="kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn GetProcessTimes(process: *mut c_void, creation: *mut FileTime, exit: *mut FileTime,
                           kernel: *mut FileTime, user: *mut FileTime) -> i32;
    }
    let mut create = FileTime { low: 0, high: 0 };
    let mut exit = FileTime { low: 0, high: 0 };
    let mut kernel = FileTime { low: 0, high: 0 };
    let mut user = FileTime { low: 0, high: 0 };
    let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut create, &mut exit, &mut kernel, &mut user) };
    if ok == 0 { return Err("GetProcessTimes failed".into()); }
    let ticks = |t: FileTime| ((t.high as u64) << 32) | t.low as u64;
    Ok((ticks(kernel) + ticks(user)) as f64 / 10_000_000.0)
}

fn read_wav(path: &Path) -> Result<Vec<f32>, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    if spec.channels != 1 || spec.sample_rate != RATE || spec.bits_per_sample != 16 || spec.sample_format != hound::SampleFormat::Int {
        return Err("WAV must be mono 16-bit at 48000 Hz".into());
    }
    reader.samples::<i16>().map(|x| x.map(|x| x as f32 / 32768.0)).collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}
fn run() -> Result<(), String> {
    let args: Vec<_> = env::args_os().collect();
    if args.len() != 5 { return Err("usage: cpu_benchmark reference.wav input.wav model_dir gtcrn|dpdfnet2".into()); }
    let mode = args[4].to_str().ok_or("Invalid mode")?;
    if mode != "gtcrn" && mode != "dpdfnet2" { return Err("Invalid mode".into()); }
    let reference = read_wav(Path::new(&args[1]))?;
    let input = read_wav(Path::new(&args[2]))?;
    let warm = 25 * BLOCK;
    let count = 375 * BLOCK;
    if input.len() < warm + count { return Err("Need at least 32 seconds of input".into()); }
    let mut processor = Processor::new(Path::new(&args[3]), &reference, RATE, mode == "dpdfnet2")?;
    for chunk in input[..warm].chunks_exact(BLOCK) { processor.process(chunk)?; }
    let mut out = vec![0.0f32; count];
    for trial in 1..=3 {
        let cpu0 = process_cpu_seconds()?;
        let start = Instant::now();
        for (src, dst) in input[warm..warm+count].chunks_exact(BLOCK).zip(out.chunks_exact_mut(BLOCK)) {
            dst.copy_from_slice(&processor.process(src)?);
        }
        let wall = start.elapsed().as_secs_f64();
        let cpu = process_cpu_seconds()? - cpu0;
        if out.iter().any(|x| !x.is_finite()) { return Err("Non-finite output".into()); }
        let rms = (out.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / count as f64).sqrt();
        println!("{{\"mode\":\"{}\",\"trial\":{},\"audio_seconds\":30,\"wall_seconds\":{:.6},\"cpu_seconds\":{:.6},\"cpu_one_core_percent\":{:.3},\"realtime_factor\":{:.6},\"output_rms\":{:.10}}}", mode, trial, wall, cpu, cpu / 30.0 * 100.0, wall / 30.0, rms);
    }
    Ok(())
}
fn main() { if let Err(e) = run() { eprintln!("{e}"); std::process::exit(1); } }
