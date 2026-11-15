//! Native streaming speaker isolation. Uses sherpa-onnx 2.0.1's C ABI.
//! One `Processor` belongs to one audio worker; it must never be shared between
//! callbacks. The model inference runs on one CPU thread.
use libloading::Library;
use std::{collections::VecDeque, ffi::{c_char, c_void, CStr, CString}, io::{Read, Write}, path::{Path, PathBuf}, ptr, slice, sync::{Arc, atomic::{AtomicU64, Ordering}}};
use std::os::windows::fs::OpenOptionsExt;
#[path = "gpu_worker.rs"]
#[allow(dead_code)]
mod gpu_worker;

/// Return original DXGI indices, adapter LUIDs and display names.
pub fn list_gpu_adapters() -> Result<Vec<(u32, u64, String)>, String> {
    gpu_worker::list_gpu_adapters()
}

#[repr(C)]
#[derive(Default)]
struct Gtcrn { model: *const c_char }
#[repr(C)]
#[derive(Default)]
struct Dpdf { model: *const c_char, attenuation_limit_db: f32 }
#[repr(C)]
#[derive(Default)]
struct Vox { enroll_model: *const c_char, chunk_model: *const c_char, voice_sample: *const c_char }
#[repr(C)]
#[derive(Default)]
struct Model { gtcrn: Gtcrn, num_threads: i32, debug: i32, provider: *const c_char, dpdfnet: Dpdf, voxlock: Vox }
#[repr(C)]
struct Config { model: Model }
#[repr(C)]
struct Audio { samples: *const f32, n: i32, sample_rate: i32 }
type Handle = *const c_void;
struct Api {
    _library: Library,
    create: unsafe extern "C" fn(*const Config) -> Handle,
    destroy: unsafe extern "C" fn(Handle),
    rate: unsafe extern "C" fn(Handle) -> i32,
    shift: unsafe extern "C" fn(Handle) -> i32,
    set_voice: Option<unsafe extern "C" fn(Handle, *const f32, i32, i32) -> i32>,
    reset: unsafe extern "C" fn(Handle),
    run: unsafe extern "C" fn(Handle, *const f32, i32, i32) -> *const Audio,
    free_audio: unsafe extern "C" fn(*const Audio),
}
impl Api {
    fn load() -> Result<Arc<Self>, String> {
        // The DLL and its onnxruntime dependencies should be beside the exe,
        // or discoverable through the Windows DLL search path.
        let dll_path = std::env::current_exe().map_err(|e| format!("无法定位程序目录：{e}"))?
            .parent().ok_or("无法定位程序目录")?.join("runtime").join("sherpa-onnx-c-api.dll");
        // Prevent replacement between hashing and loading the DLL. Only the
        // known official 2.0.1 build has the older fallback semantics below.
        let mut dll_file = std::fs::OpenOptions::new().read(true).share_mode(1).open(&dll_path)
            .map_err(|e| format!("无法打开 sherpa-onnx 运行库：{e}"))?;
        // Also search the DLL directory for transitive dependencies.
        let library: Library = unsafe { libloading::os::windows::Library::load_with_flags(&dll_path, 0x00000100 | 0x00001000) }.map(Into::into)
            .map_err(|e| format!("无法加载 sherpa-onnx-c-api.dll（需 2.0.1 及配套 DLL）：{e}"))?;
        unsafe {
            macro_rules! sym { ($name:literal) => { *library.get(concat!($name, "\0").as_bytes()).map_err(|e| format!("sherpa-onnx 版本不兼容，缺少 {}: {e}", $name))? }; }
            let version: unsafe extern "C" fn() -> *const c_char = sym!("SherpaOnnxGetVersionStr");
            let version_ptr = version();
            if version_ptr.is_null() || CStr::from_ptr(version_ptr).to_bytes() != b"2.0.1" {
                return Err("sherpa-onnx 运行库版本不匹配，需要 2.0.1".into());
            }
            let set_voice = library.get(b"SherpaOnnxOnlineSpeechDenoiserSetVoiceSample\0").ok().map(|symbol| *symbol);
            if set_voice.is_none() {
                let mut bytes = Vec::new();
                dll_file.read_to_end(&mut bytes).map_err(|e| format!("无法校验运行库：{e}"))?;
                const OFFICIAL: [u8; 32] = [0x71,0x72,0x8f,0xb1,0x2b,0x3a,0x15,0xa9,0xab,0x7c,0x49,0x61,0xfc,0xea,0xfc,0x6d,0xd8,0x02,0xf7,0xfc,0x03,0x8f,0x46,0x1f,0x5e,0xa5,0x9e,0x07,0xd7,0x10,0x4b,0xec];
                if sha256(&bytes)? != OFFICIAL { return Err("运行库缺少声样接口，且不是已验证的官方 2.0.1 版本".into()); }
            }
            Ok(Arc::new(Self { create: sym!("SherpaOnnxCreateOnlineSpeechDenoiser"), destroy: sym!("SherpaOnnxDestroyOnlineSpeechDenoiser"), rate: sym!("SherpaOnnxOnlineSpeechDenoiserGetSampleRate"), shift: sym!("SherpaOnnxOnlineSpeechDenoiserGetFrameShiftInSamples"), set_voice, reset: sym!("SherpaOnnxOnlineSpeechDenoiserReset"), run: sym!("SherpaOnnxOnlineSpeechDenoiserRun"), free_audio: sym!("SherpaOnnxDestroyDenoisedAudio"), _library: library }))
        }
    }
}

// Hash the fallback DLL with Windows CNG; this pins the behavior of the
// one-sample enrollment check to the exact upstream binary we inspected.
fn sha256(input: &[u8]) -> Result<[u8; 32], String> {
    #[link(name = "bcrypt")]
    extern "system" {
        fn BCryptOpenAlgorithmProvider(handle: *mut *mut c_void, alg: *const u16, implementation: *const u16, flags: u32) -> i32;
        fn BCryptHash(handle: *mut c_void, secret: *const u8, secret_len: u32, input: *const u8, input_len: u32, output: *mut u8, output_len: u32) -> i32;
        fn BCryptCloseAlgorithmProvider(handle: *mut c_void, flags: u32) -> i32;
    }
    if input.len() > u32::MAX as usize { return Err("运行库文件过大".into()); }
    let mut handle = ptr::null_mut();
    let name: Vec<u16> = "SHA256\0".encode_utf16().collect();
    if unsafe { BCryptOpenAlgorithmProvider(&mut handle, name.as_ptr(), ptr::null(), 0) } < 0 { return Err("无法初始化运行库校验".into()); }
    let mut digest = [0u8; 32];
    let result = unsafe { BCryptHash(handle, ptr::null(), 0, input.as_ptr(), input.len() as u32, digest.as_mut_ptr(), digest.len() as u32) };
    unsafe { BCryptCloseAlgorithmProvider(handle, 0); }
    if result < 0 { return Err("无法校验运行库".into()); }
    Ok(digest)
}

struct TemporaryReference { path: PathBuf, file: Option<std::fs::File> }
impl TemporaryReference {
    fn create(samples: &[f32], sample_rate: u32) -> Result<Self, String> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        for _ in 0..100 {
            let name = format!("voice-isolation-{}-{}.wav", std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed));
            let path = std::env::temp_dir().join(name);
            // Permit the library to read the file while prohibiting writes or
            // deletion until it has made its private in-memory copy.
            match std::fs::OpenOptions::new().write(true).create_new(true).share_mode(1).open(&path) {
                Ok(file) => {
                    let mut temp = Self { path, file: Some(file) };
                    let spec = hound::WavSpec { channels: 1, sample_rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
                    {
                        let file = temp.file.as_mut().unwrap();
                        let mut wav = hound::WavWriter::new(file, spec).map_err(|e| format!("无法准备临时声样：{e}"))?;
                        let max = (sample_rate as usize).saturating_mul(30).min(samples.len());
                        for &sample in &samples[..max] { wav.write_sample((sample.clamp(-1.0, 1.0) * 32767.0) as i16).map_err(|e| e.to_string())?; }
                        wav.finalize().map_err(|e| e.to_string())?;
                    }
                    temp.file.as_mut().unwrap().flush().map_err(|e| e.to_string())?;
                    return Ok(temp);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("无法准备临时声样：{e}")),
            }
        }
        Err("无法创建临时声样文件".into())
    }
}
impl Drop for TemporaryReference {
    fn drop(&mut self) { drop(self.file.take()); let _ = std::fs::remove_file(&self.path); }
}
struct Stage { api: Arc<Api>, handle: Handle, pending_samples: usize }
impl Stage {
    fn create(api: Arc<Api>, model: Model) -> Result<Self, String> {
        let handle = unsafe { (api.create)(&Config { model }) };
        if handle.is_null() { return Err("创建语音模型失败，请检查模型和运行库版本".into()); }
        let stage = Self { api, handle, pending_samples: 0 };
        if unsafe { (stage.api.rate)(handle) } != 16000 { return Err("模型采样率不是 16000 Hz".into()); }
        Ok(stage)
    }
    fn feed(&mut self, samples: &[f32]) -> Result<Vec<f32>, String> {
        if samples.len() > i32::MAX as usize { return Err("音频块过大".into()); }
        let p = unsafe { (self.api.run)(self.handle, samples.as_ptr(), samples.len() as i32, 16000) };
        // The upstream API explicitly permits NULL while accumulating input.
        // Treat a prolonged lack of output as a fault, rather than silently
        // producing unlimited silence if the model has stopped making progress.
        self.pending_samples = self.pending_samples.saturating_add(samples.len());
        if p.is_null() {
            return if self.pending_samples > 5 * 16000 { Err("模型持续没有输出，请检查模型和运行库".into()) } else { Ok(Vec::new()) };
        }
        let out = unsafe {
            let audio = &*p;
            let result = if audio.n < 0 || audio.sample_rate != 16000 || (audio.n > 0 && audio.samples.is_null()) {
                Err("模型返回了无效音频".to_string())
            } else if audio.n == 0 { Ok(Vec::new()) }
            else { Ok(slice::from_raw_parts(audio.samples, audio.n as usize).to_vec()) };
            (self.api.free_audio)(p);
            result
        };
        if let Ok(ref samples) = out {
            if samples.iter().any(|sample| !sample.is_finite()) { return Err("模型返回了无效音频样本".into()); }
            if !samples.is_empty() { self.pending_samples = 0; }
            else if self.pending_samples > 5 * 16000 { return Err("模型持续没有输出，请检查模型和运行库".into()); }
        }
        out
    }
    fn shift(&self) -> usize { unsafe { (self.api.shift)(self.handle).max(0) as usize } }
    fn confirm_preloaded_voice(&mut self) -> Result<(), String> {
        // The pinned upstream build passes an input sample through when its
        // WAV enrollment failed. A successfully enrolled VoxLock stream must
        // buffer this one sample (the STFT needs >256 samples). Never forward
        // probe data, and reject *any* returned audio before live input starts.
        if self.shift() != 128 { return Err("VoxLock 模型的帧长不匹配".into()); }
        let probe = [0.3125f32];
        let returned = unsafe { (self.api.run)(self.handle, probe.as_ptr(), 1, 16000) };
        if !returned.is_null() {
            unsafe { (self.api.free_audio)(returned); }
            return Err("声样未成功录入，已阻止原始声音通过，请重新录制".into());
        }
        // The inspected upstream Reset recreates the stream using its cached
        // voice_sample_, so no microphone data can become the enrollment.
        unsafe { (self.api.reset)(self.handle); }
        Ok(())
    }
}
impl Drop for Stage { fn drop(&mut self) { unsafe { (self.api.destroy)(self.handle) } } }
enum Extractor {
    Cpu(Stage),
    Gpu(gpu_worker::GpuExtractor),
}
impl Extractor {
    fn feed(&mut self, samples: &[f32]) -> Result<Vec<f32>, String> {
        match self {
            Self::Cpu(stage) => stage.feed(samples),
            Self::Gpu(stage) => stage.feed(samples),
        }
    }
}
fn path_string(path: &Path) -> Result<CString, String> {
    if !path.is_file() { return Err(format!("缺少模型文件：{}", path.display())); }
    CString::new(path.to_string_lossy().as_bytes()).map_err(|_| "模型路径包含无效字符".into())
}

/// 48 kHz mono PCM in and out. Reference PCM must contain at least 3 seconds.
pub struct Processor {
    extractor: Extractor,
    denoiser: Stage,
    extract_queue: VecDeque<f32>,
    denoise_queue: VecDeque<f32>,
    filter: [f32; 97],
    down_history: [f32; 97],
    down_pos: usize,
    up_history: [f32; 33],
    up_pos: usize,
}
// Exclusive ownership and &mut self prevent concurrent use of the C handles.
// Moving the processor between threads does not share its state.
unsafe impl Send for Processor {}
impl Processor {
    pub fn new(model_dir: &Path, reference: &[f32], reference_rate: u32, strong_music: bool) -> Result<Self, String> {
        if reference_rate == 0 || reference_rate > i32::MAX as u32 || reference.len() < reference_rate as usize * 3 || reference.len() > i32::MAX as usize {
            return Err("声样至少需要 3 秒".into());
        }
        let rms = (reference.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / reference.len() as f64).sqrt();
        if !rms.is_finite() || rms < 0.001 { return Err("声样音量过低或无效，请重新录制".into()); }
        let api = Api::load()?;
        let enroll = path_string(&model_dir.join("enroll.onnx"))?;
        let chunk = path_string(&model_dir.join("chunk_int8.onnx"))?;
        let cpu = CString::new("cpu").unwrap();
        let temporary = if api.set_voice.is_none() { Some(TemporaryReference::create(reference, reference_rate)?) } else { None };
        let wav_path = temporary.as_ref().map(|wav| path_string(&wav.path)).transpose()?;
        let mut extractor = Stage::create(api.clone(), Model { num_threads: 1, provider: cpu.as_ptr(), voxlock: Vox { enroll_model: enroll.as_ptr(), chunk_model: chunk.as_ptr(), voice_sample: wav_path.as_ref().map_or(ptr::null(), |p| p.as_ptr()) }, ..Default::default() })?;
        if let Some(set_voice) = api.set_voice {
            let rc = unsafe { set_voice(extractor.handle, reference.as_ptr(), reference.len() as i32, reference_rate as i32) };
            if rc != 0 { return Err("声样录入失败，请重新录制".into()); }
        } else {
            extractor.confirm_preloaded_voice()?;
        }
        drop(temporary);
        let denoiser = Self::create_denoiser(api, model_dir, strong_music)?;
        Ok(Self::with_stages(Extractor::Cpu(extractor), denoiser))
    }

    /// Start a separately isolated GPU extractor; the denoiser remains CPU.
    /// `reference16k` is explicitly prepared from the user's enrolled WAV,
    /// never inferred from live microphone input.
    pub fn new_gpu(model_dir: &Path, reference16k: &[f32], block_size_16k: usize, strong_music: bool, adapter: Option<(u32, u64)>) -> Result<Self, String> {
        let api = Api::load()?;
        let extractor = gpu_worker::GpuExtractor::new(reference16k, model_dir, block_size_16k, adapter)?;
        let denoiser = Self::create_denoiser(api, model_dir, strong_music)?;
        Ok(Self::with_stages(Extractor::Gpu(extractor), denoiser))
    }

    #[allow(dead_code)] // Used by the standalone benchmark, not the GUI.
    pub fn gpu_device_name(&self) -> Option<&str> {
        match &self.extractor { Extractor::Gpu(stage) => Some(stage.device_name()), Extractor::Cpu(_) => None }
    }

    pub fn gpu_device_description(&self) -> Option<String> {
        match &self.extractor {
            Extractor::Gpu(stage) => Some(format!("{}（DXGI 设备 {}）", stage.device_name(), stage.adapter_index())),
            Extractor::Cpu(_) => None,
        }
    }

    /// CPU time of the isolated GPU worker, for explicit benchmark accounting.
    #[allow(dead_code)]
    pub fn gpu_worker_cpu_seconds(&self) -> Result<f64, String> {
        match &self.extractor { Extractor::Gpu(stage) => stage.cpu_seconds(), Extractor::Cpu(_) => Ok(0.0) }
    }

    #[allow(dead_code)]
    pub fn gpu_worker_process_id(&self) -> Option<u32> {
        match &self.extractor { Extractor::Gpu(stage) => Some(stage.process_id()), Extractor::Cpu(_) => None }
    }

    fn create_denoiser(api: Arc<Api>, model_dir: &Path, strong_music: bool) -> Result<Stage, String> {
        let noise = path_string(&model_dir.join(if strong_music { "dpdfnet2.onnx" } else { "gtcrn_simple.onnx" }))?;
        let cpu = CString::new("cpu").unwrap();
        let mut denoise_model = Model { num_threads: 1, provider: cpu.as_ptr(), ..Default::default() };
        if strong_music { denoise_model.dpdfnet.model = noise.as_ptr(); } else { denoise_model.gtcrn.model = noise.as_ptr(); }
        Stage::create(api, denoise_model)
    }

    fn with_stages(extractor: Extractor, denoiser: Stage) -> Self {
        let denoise_delay = denoiser.shift();
        let mut filter = [0.0f32; 97];
        let mut sum = 0.0f64;
        for (i, f) in filter.iter_mut().enumerate() {
            let n = i as f64 - 48.0;
            let sinc = if n == 0.0 { 0.30 } else { (std::f64::consts::PI * 0.30 * n).sin() / (std::f64::consts::PI * n) };
            let window = 0.54 - 0.46 * (2.0 * std::f64::consts::PI * i as f64 / 96.0).cos();
            *f = (sinc * window) as f32;
            sum += *f as f64;
        }
        for f in &mut filter { *f /= sum as f32; }
        Self { extractor, denoiser, extract_queue: vec![0.0; 512].into(), denoise_queue: vec![0.0; denoise_delay].into(), filter, down_history: [0.0; 97], down_pos: 0, up_history: [0.0; 33], up_pos: 0 }
    }
    /// Blocks must contain a multiple of three samples so the 48/16 kHz phase stays aligned.
    pub fn process(&mut self, input: &[f32]) -> Result<Vec<f32>, String> {
        if input.len() % 3 != 0 { return Err("音频块长度必须是 3 的倍数".into()); }
        let mut down = Vec::with_capacity(input.len() / 3);
        for (i, &s) in input.iter().enumerate() {
            self.down_history[self.down_pos] = s;
            if i % 3 == 0 {
                let mut y = 0.0;
                for k in 0..97 { y += self.filter[k] * self.down_history[(self.down_pos + 97 - k) % 97]; }
                down.push(y);
            }
            self.down_pos = (self.down_pos + 1) % 97;
        }
        let extracted = self.extractor.feed(&down)?;
        let selected = take_ordered(&mut self.extract_queue, extracted, down.len());
        let enhanced = self.denoiser.feed(&selected)?;
        let selected = take_ordered(&mut self.denoise_queue, enhanced, down.len());
        let mut out = Vec::with_capacity(input.len());
        for s in selected {
            self.up_history[self.up_pos] = s * 3.0;
            for phase in 0..3 {
                let mut y = 0.0;
                let mut k = phase;
                while k < 97 {
                    y += self.filter[k] * self.up_history[(self.up_pos + 33 - k / 3) % 33];
                    k += 3;
                }
                out.push(y);
            }
            self.up_pos = (self.up_pos + 1) % 33;
        }
        Ok(out)
    }
}
fn take_ordered(queue: &mut VecDeque<f32>, ready: Vec<f32>, count: usize) -> Vec<f32> {
    queue.extend(ready);
    let missing = count.saturating_sub(queue.len());
    let mut output = Vec::with_capacity(count);
    output.resize(missing, 0.0);
    for _ in missing..count { output.push(queue.pop_front().unwrap()); }
    output
}
