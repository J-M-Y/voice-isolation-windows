//! Device-free, paced full Rust pipeline measurement using explicitly supplied WAVs.
//! Includes the isolated GPU worker's CPU time. Does not open any audio device.
#![allow(dead_code)]
#[path = "../audio.rs"] mod audio;
#[path = "../processor.rs"] mod processor;
use std::{env, path::Path, thread, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};

#[cfg(windows)]
fn cpu_time() -> Result<f64, String> {
    use std::ffi::c_void;
    #[repr(C)] #[derive(Default)] struct FileTime { low: u32, high: u32 }
    #[link(name="kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn GetProcessTimes(process: *mut c_void, creation: *mut FileTime, exit: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
    }
    let (mut creation, mut exit, mut kernel, mut user) = (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
    if unsafe { GetProcessTimes(GetCurrentProcess(), &mut creation, &mut exit, &mut kernel, &mut user) } == 0 { return Err("GetProcessTimes failed".into()); }
    let ticks = |t: FileTime| ((t.high as u64) << 32) | t.low as u64;
    Ok((ticks(kernel) + ticks(user)) as f64 / 10_000_000.0)
}

fn read_wav(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    if spec.channels != 1 || spec.bits_per_sample != 16 || spec.sample_format != hound::SampleFormat::Int { return Err("Expected mono 16-bit PCM WAV".into()); }
    let samples = reader.samples::<i16>().map(|v| v.map(|x| x as f32 / 32768.0)).collect::<Result<Vec<_>,_>>().map_err(|e| e.to_string())?;
    Ok((samples, spec.sample_rate))
}

fn run() -> Result<(), String> {
    let args: Vec<_> = env::args_os().collect();
    if args.len() == 2 && args[1] == "--list-adapters" {
        for (index, luid, name) in processor::list_gpu_adapters()? {
            println!("{index}\t{luid:016X}\t{name}");
        }
        return Ok(());
    }
    if ![8, 9, 11].contains(&args.len()) { return Err("usage: realtime_compare <reference.wav> <input48k.wav> <models> <output.wav> <cpu|gpu> <normal|strong> <80|160|240> [duration_ms [adapter_index adapter_luid_hex]]".into()); }
    let mode = args[5].to_str().ok_or("invalid provider")?;
    if mode != "cpu" && mode != "gpu" { return Err("invalid provider".into()); }
    let noise = args[6].to_str().ok_or("invalid denoiser")?;
    if noise != "normal" && noise != "strong" { return Err("invalid denoiser".into()); }
    let ms: usize = args[7].to_str().ok_or("invalid block")?.parse().map_err(|_| "invalid block")?;
    if ![80, 160, 240].contains(&ms) { return Err("invalid block".into()); }
    let block = ms * 48;
    let (reference, ref_rate) = read_wav(Path::new(&args[1]))?;
    let (input, rate) = read_wav(Path::new(&args[2]))?;
    if rate != 48_000 { return Err("input must be 48000 Hz".into()); }
    let warm = 2_400 * 48;
    let duration_ms: usize = if args.len() >= 9 { args[8].to_str().ok_or("invalid duration")?.parse().map_err(|_| "invalid duration")? } else { 30_240 };
    if duration_ms == 0 || duration_ms > 300_000 || duration_ms % ms != 0 { return Err("duration must be a positive whole number of blocks (max 300000 ms)".into()); }
    let count = duration_ms * 48;
    let audio_seconds = duration_ms as f64 / 1000.0;
    if input.len() < warm + count || warm % block != 0 { return Err("input must contain 2.4s warmup plus the requested duration".into()); }
    let init = Instant::now();
    let adapter = if args.len() == 11 {
        if mode != "gpu" { return Err("adapter selection requires GPU mode".into()); }
        let index = args[9].to_str().ok_or("invalid adapter index")?.parse::<u32>().map_err(|_| "invalid adapter index")?;
        let luid = u64::from_str_radix(args[10].to_str().ok_or("invalid adapter LUID")?, 16).map_err(|_| "invalid adapter LUID")?;
        Some((index, luid))
    } else { None };
    let mut processor = if mode == "gpu" {
        let sample = audio::gpu_reference(&reference, ref_rate)?;
        processor::Processor::new_gpu(Path::new(&args[3]), &sample, block / 3, noise == "strong", adapter)?
    } else { processor::Processor::new(Path::new(&args[3]), &reference, ref_rate, noise == "strong")? };
    let init_seconds = init.elapsed().as_secs_f64();
    let device = processor.gpu_device_name().unwrap_or("CPU").to_string();
    let worker_pid = processor.gpu_worker_process_id().unwrap_or(0);
    for chunk in input[..warm].chunks_exact(block) { processor.process(chunk)?; }
    let cpu0 = cpu_time()? + processor.gpu_worker_cpu_seconds()?;
    let measurement_start_unix_s = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_secs_f64();
    let start = Instant::now();
    let period = Duration::from_millis(ms as u64);
    let mut times = Vec::new();
    let mut packet_delays = Vec::new();
    let mut next_playback: Option<Instant> = None;
    let mut late = 0usize;
    let mut output = Vec::with_capacity(count);
    for (i, chunk) in input[warm..warm + count].chunks_exact(block).enumerate() {
        // A block becomes available only after its capture interval ends.
        let capture_start = start + period * (i as u32);
        let scheduled = capture_start + period;
        if let Some(wait) = scheduled.checked_duration_since(Instant::now()) { thread::sleep(wait); }
        let begin = Instant::now();
        let result = processor.process(chunk)?;
        if result.len() != block || !result.iter().all(|v| v.is_finite()) { return Err("invalid output".into()); }
        output.extend(result);
        let end = Instant::now();
        times.push(end.duration_since(begin).as_secs_f64() * 1000.0);
        if end > scheduled + period { late += 1; }
        // No-drop playback simulation: preserve the queued audio in order.
        // This is packet delay, not a real sound-card round-trip measurement.
        let playback = next_playback.map_or(end, |previous| previous.max(end));
        packet_delays.push(playback.duration_since(capture_start).as_secs_f64() * 1000.0);
        next_playback = Some(playback + period);
    }
    let elapsed = start.elapsed().as_secs_f64();
    let measurement_end_unix_s = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_secs_f64();
    let cpu = cpu_time()? + processor.gpu_worker_cpu_seconds()? - cpu0;
    let rms = (output.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / output.len() as f64).sqrt();
    if !rms.is_finite() || rms < 1e-8 { return Err("output is silent or invalid".into()); }
    let spec = hound::WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut wav = hound::WavWriter::create(Path::new(&args[4]), spec).map_err(|e| e.to_string())?;
    for x in output { wav.write_sample(x).map_err(|e| e.to_string())?; }
    wav.finalize().map_err(|e| e.to_string())?;
    times.sort_by(f64::total_cmp);
    let percentile = |p: f64| times[((times.len() - 1) as f64 * p).round() as usize];
    packet_delays.sort_by(f64::total_cmp);
    let delay = |p: f64| packet_delays[((packet_delays.len() - 1) as f64 * p).round() as usize];
    println!("{{\"provider\":\"{mode}\",\"denoiser\":\"{noise}\",\"block_ms\":{ms},\"device\":{:?},\"worker_pid\":{worker_pid},\"init_seconds\":{init_seconds:.4},\"audio_seconds\":{audio_seconds:.3},\"elapsed_seconds\":{elapsed:.4},\"measurement_start_unix_s\":{measurement_start_unix_s:.3},\"measurement_end_unix_s\":{measurement_end_unix_s:.3},\"cpu_seconds_parent_plus_worker\":{cpu:.4},\"cpu_one_core_percent\":{:.2},\"processing_p50_ms\":{:.3},\"processing_p95_ms\":{:.3},\"processing_p99_ms\":{:.3},\"capture_to_packet_p50_ms\":{:.3},\"capture_to_packet_p95_ms\":{:.3},\"late_blocks\":{late},\"blocks\":{},\"output_rms\":{rms:.9}}}", device, cpu / audio_seconds * 100.0, percentile(0.50), percentile(0.95), percentile(0.99), delay(0.50), delay(0.95), times.len());
    Ok(())
}
fn main() { if let Err(error) = run() { eprintln!("{error}"); std::process::exit(1); } }
