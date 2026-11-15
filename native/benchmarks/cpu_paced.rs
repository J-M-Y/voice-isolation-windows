//! Paged synthetic-fixture CPU reference, the actual production Rust Processor.
//! It does not open any physical audio device. Compile beside processor.rs in
//! a temporary Cargo project and run one fresh process per trial.
#[path = "processor.rs"]
mod processor;
use processor::Processor;
use std::{env, path::Path, thread, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
const RATE: u32 = 48_000;
const WARM: usize = 115_200;
const MEASURED: usize = 1_451_520; // 30.24 s; divisible by 80/160/240 ms blocks.
#[repr(C)] struct FileTime { low:u32, high:u32 }
#[link(name="kernel32")]
extern "system" {
    fn GetCurrentProcess()->*mut std::ffi::c_void;
    fn GetProcessTimes(p:*mut std::ffi::c_void,c:*mut FileTime,e:*mut FileTime,k:*mut FileTime,u:*mut FileTime)->i32;
}
fn cpu()->Result<f64,String> {
    let(mut c,mut e,mut k,mut u)=(FileTime{low:0,high:0},FileTime{low:0,high:0},FileTime{low:0,high:0},FileTime{low:0,high:0});
    if unsafe{GetProcessTimes(GetCurrentProcess(),&mut c,&mut e,&mut k,&mut u)}==0{return Err("GetProcessTimes failed".into());}
    let s=|t:FileTime| (((t.high as u64)<<32)|t.low as u64) as f64/10_000_000.0;
    Ok(s(k)+s(u))
}
fn read(path:&Path)->Result<(Vec<f32>,u32),String>{
    let mut wav=hound::WavReader::open(path).map_err(|e|e.to_string())?;let spec=wav.spec();
    if spec.channels!=1 || spec.bits_per_sample!=16 || spec.sample_format!=hound::SampleFormat::Int || ![16_000,44_100,48_000].contains(&spec.sample_rate){return Err("expected mono 16-bit PCM, 16k, 44.1k or 48k".into());}
    let out=wav.samples::<i16>().map(|x|x.map(|v|v as f32/32768.)).collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
    Ok((out,spec.sample_rate))
}
fn percentile(data:&[f64],p:f64)->f64{
    let mut sorted=data.to_vec();sorted.sort_by(|a,b|a.total_cmp(b));let at=(sorted.len()-1) as f64*p/100.;let a=at.floor() as usize;let b=at.ceil() as usize;
    sorted[a]+(sorted[b]-sorted[a])*(at-a as f64)
}
fn main_impl()->Result<(),String>{
    let a:Vec<_>=env::args_os().collect();
    if a.len()!=8{return Err("usage: cpu_paced reference.wav input.wav models block_samples normal|strong output.wav result.json".into());}
    let (reference,ref_rate)=read(Path::new(&a[1]))?;let (input,input_rate)=read(Path::new(&a[2]))?;
    let block:usize=a[4].to_str().ok_or("block")?.parse().map_err(|_|"invalid block")?;
    let strong=match a[5].to_str(){Some("normal")=>false,Some("strong")=>true,_=>return Err("invalid mode".into())};
    if input_rate!=RATE || ![3840,7680,11520].contains(&block) || input.len()<WARM+MEASURED{return Err("invalid block or input too short".into());}
    let t0=Instant::now();let mut model=Processor::new(Path::new(&a[3]),&reference,ref_rate,strong)?;let init=t0.elapsed().as_secs_f64();
    let w0=Instant::now();for chunk in input[..WARM].chunks_exact(block){model.process(chunk)?;}let warm=w0.elapsed().as_secs_f64();
    let mut output=Vec::with_capacity(MEASURED);let(mut block_times,mut late,mut playback_delays)=(Vec::new(),Vec::new(),Vec::new());
    let block_s=block as f64 / RATE as f64;let mut playback_end=0.;let start_cpu=cpu()?;
    let unix=SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e|e.to_string())?.as_secs_f64();let begin=Instant::now();
    for (i,chunk) in input[WARM..WARM+MEASURED].chunks_exact(block).enumerate(){
        let deadline=Duration::from_secs_f64((i+1) as f64*block_s);if let Some(delay)=deadline.checked_sub(begin.elapsed()){thread::sleep(delay);}
        let t=Instant::now();let result=model.process(chunk)?;let elapsed=t.elapsed().as_secs_f64()*1000.;let done=begin.elapsed().as_secs_f64();
        if result.len()!=block || result.iter().any(|s|!s.is_finite()){return Err("invalid model output".into());}
        output.extend(result);block_times.push(elapsed);late.push(((done-((i+2) as f64*block_s)).max(0.))*1000.);
        let play_start=done.max(playback_end);playback_delays.push((play_start-i as f64*block_s)*1000.);playback_end=play_start+block_s;
    }
    let wall=begin.elapsed().as_secs_f64();let used=cpu()?-start_cpu;
    let rms=(output.iter().map(|&x|(x as f64).powi(2)).sum::<f64>()/output.len() as f64).sqrt();
    let spec=hound::WavSpec{channels:1,sample_rate:RATE,bits_per_sample:32,sample_format:hound::SampleFormat::Float};
    let mut writer=hound::WavWriter::create(Path::new(&a[6]),spec).map_err(|e|e.to_string())?;for &s in &output{writer.write_sample(s).map_err(|e|e.to_string())?;}writer.finalize().map_err(|e|e.to_string())?;
    let text=format!("{{\"provider\":\"production_cpu\",\"mode\":\"{}\",\"block_ms\":{},\"input_seconds\":30.24,\"warmup_seconds\":2.4,\"wall_seconds\":{:.6},\"measurement_start_unix_s\":{:.6},\"initialization_seconds\":{:.6},\"warm_wall_seconds\":{:.6},\"parent_cpu_seconds\":{:.6},\"child_cpu_seconds\":0,\"combined_cpu_seconds\":{:.6},\"combined_single_core_percent\":{:.6},\"process_ms_p50\":{:.6},\"process_ms_p95\":{:.6},\"process_ms_p99\":{:.6},\"deadline_misses\":{},\"block_count\":{},\"deadline_late_ms_max\":{:.6},\"simulated_capture_to_packet_play_ms_p50\":{:.6},\"simulated_capture_to_packet_play_ms_p95\":{:.6},\"simulated_capture_to_packet_play_ms_max\":{:.6},\"output_rms\":{:.9},\"scope\":\"production Rust Processor; no audio devices or UI; no-drop simulated playback\"}}",if strong{"strong"}else{"normal"},block*1000/RATE as usize,wall,unix,init,warm,used,used,used/30.24*100.,percentile(&block_times,50.),percentile(&block_times,95.),percentile(&block_times,99.),late.iter().filter(|&&x|x>0.).count(),late.len(),late.iter().copied().fold(0.,f64::max),percentile(&playback_delays,50.),percentile(&playback_delays,95.),playback_delays.iter().copied().fold(0.,f64::max),rms);
    std::fs::write(Path::new(&a[7]),&text).map_err(|e|e.to_string())?;println!("{text}");Ok(())
}
fn main(){if let Err(e)=main_impl(){eprintln!("{e}");std::process::exit(1);}}
