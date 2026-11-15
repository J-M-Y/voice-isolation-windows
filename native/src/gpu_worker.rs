//! DirectML worker client. This module does not bypass isolation:
//! any protocol error, timeout or worker failure is an error, never raw audio.
//! The child is confined to a Windows job so it is killed if the parent exits.
use std::{ffi::c_void, io::{Read, Write}, os::windows::{io::AsRawHandle, process::CommandExt}, path::Path,
    process::{Child, Command, Stdio}, ptr, sync::mpsc::{self, Receiver, SyncSender}, thread::{self, JoinHandle}, time::{Duration, Instant}};

const REFERENCE_SAMPLES: usize = 48_000;
const ERROR_SENTINEL: u32 = u32::MAX;
const PROTOCOL_MAGIC: u32 = 0x3255_5047; // GPU2
const MAX_ERROR: usize = 4096;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
const START_TIMEOUT: Duration = Duration::from_secs(30);
const BLOCK_TIMEOUT: Duration = Duration::from_secs(3);
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
const ENUM_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ENUM_BYTES: u64 = 512 * 1024;

#[repr(C)]
#[derive(Default)]
struct IoCounters { read_ops: u64, write_ops: u64, other_ops: u64, read_bytes: u64, write_bytes: u64, other_bytes: u64 }
#[repr(C)]
#[derive(Default)]
struct BasicLimits {
    process_time_limit: i64, job_time_limit: i64, flags: u32,
    min_working_set: usize, max_working_set: usize, active_process_limit: u32,
    affinity: usize, priority_class: u32, scheduling_class: u32,
}
#[repr(C)]
#[derive(Default)]
struct ExtendedLimits { basic: BasicLimits, io: IoCounters, process_memory: usize, job_memory: usize, peak_process_memory: usize, peak_job_memory: usize }
#[repr(C)]
#[derive(Default)]
struct FileTime { low: u32, high: u32 }
#[link(name = "kernel32")]
extern "system" {
    fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
    fn SetInformationJobObject(job: *mut c_void, class: i32, info: *const c_void, size: u32) -> i32;
    fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
    fn TerminateJobObject(job: *mut c_void, exit_code: u32) -> i32;
    fn GetProcessTimes(process: *mut c_void, creation: *mut FileTime, exit: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}
struct KillOnCloseJob(*mut c_void);
// The job handle has unique ownership, is never used concurrently, and Windows
// permits a process/job handle to be closed on a thread other than its creator.
unsafe impl Send for KillOnCloseJob {}
impl KillOnCloseJob {
    fn attach(child: &Child) -> Result<Self, String> {
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() { return Err(format!("无法创建 GPU 进程保护：{}", std::io::Error::last_os_error())); }
        let job = Self(handle);
        let mut limits = ExtendedLimits::default();
        limits.basic.flags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        let set = unsafe { SetInformationJobObject(job.0, 9, &limits as *const _ as *const c_void, std::mem::size_of::<ExtendedLimits>() as u32) };
        if set == 0 { return Err(format!("无法设置 GPU 进程保护：{}", std::io::Error::last_os_error())); }
        if unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle() as *mut c_void) } == 0 {
            return Err(format!("无法关联 GPU 进程保护：{}", std::io::Error::last_os_error()));
        }
        Ok(job)
    }
    fn terminate(&self) { unsafe { TerminateJobObject(self.0, 1); } }
}
impl Drop for KillOnCloseJob { fn drop(&mut self) { unsafe { CloseHandle(self.0); } } }
fn kill_child_bounded(child: &mut Child) {
    let _ = child.kill();
    let until = Instant::now() + STOP_TIMEOUT;
    while Instant::now() < until {
        match child.try_wait() { Ok(Some(_)) | Err(_) => break, Ok(None) => thread::sleep(Duration::from_millis(10)) }
    }
}

fn worker_path() -> Result<std::path::PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("无法定位程序：{e}"))?;
    Ok(exe.parent().ok_or("无法定位程序目录")?.join("runtime-dml").join("vox_v1_worker_checked.exe"))
}

fn parse_adapter_list(bytes: &[u8]) -> Result<Vec<(u32, u64, String)>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "GPU 列表编码无效")?;
    let mut lines = text.lines();
    if lines.next() != Some("GPU2-LIST\t1") { return Err("GPU 设备列表协议不兼容，请重新安装 GPU 组件".into()); }
    let mut result = Vec::new();
    for line in lines {
        if result.len() >= 128 { return Err("GPU 设备列表过长".into()); }
        let parts: Vec<_> = line.split('\t').collect();
        if parts.len() != 5 || parts[1].len() != 16 || parts[2].len() != 8 || parts[3].len() != 8 || parts[4].is_empty() || parts[4].len() > 2046 || parts[4].len() % 2 != 0 {
            return Err("GPU 设备列表格式无效".into());
        }
        let index = parts[0].parse::<u32>().map_err(|_| "GPU 设备编号无效")?;
        let luid = u64::from_str_radix(parts[1], 16).map_err(|_| "GPU 设备标识无效")?;
        u32::from_str_radix(parts[2], 16).map_err(|_| "GPU 厂商标识无效")?;
        u32::from_str_radix(parts[3], 16).map_err(|_| "GPU 型号标识无效")?;
        let raw = parts[4].as_bytes().chunks_exact(2).map(|pair| {
            let value = std::str::from_utf8(pair).map_err(|_| "GPU 名称编码无效")?;
            u8::from_str_radix(value, 16).map_err(|_| "GPU 名称编码无效")
        }).collect::<Result<Vec<_>, _>>()?;
        let name = String::from_utf8(raw).map_err(|_| "GPU 名称编码无效")?;
        if name.trim().is_empty() || result.iter().any(|(old_index, old_luid, _)| *old_index == index || *old_luid == luid) {
            return Err("GPU 设备列表包含无效或重复标识".into());
        }
        result.push((index, luid, name));
    }
    Ok(result)
}

/// Enumerate DXGI hardware devices without opening a microphone or model.
/// Failures are returned independently so the CPU path remains available.
pub fn list_gpu_adapters() -> Result<Vec<(u32, u64, String)>, String> {
    let worker = worker_path()?;
    if !worker.is_file() { return Err(format!("GPU 组件不存在：{}；请先安装 GPU 组件", worker.display())); }
    let mut child = Command::new(worker).arg("--list-adapters")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null())
        .creation_flags(0x0800_0000).spawn().map_err(|e| format!("无法读取 GPU 设备：{e}"))?;
    let job = match KillOnCloseJob::attach(&child) {
        Ok(job) => job,
        Err(e) => { kill_child_bounded(&mut child); return Err(e); }
    };
    let stdout = match child.stdout.take() { Some(value) => value, None => { job.terminate(); kill_child_bounded(&mut child); return Err("无法读取 GPU 设备列表".into()); } };
    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut data = Vec::new();
        let result = stdout.take(MAX_ENUM_BYTES + 1).read_to_end(&mut data).map(|_| data);
        let _ = tx.send(result);
    });
    let until = Instant::now() + ENUM_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < until => thread::sleep(Duration::from_millis(10)),
            Ok(None) => { job.terminate(); kill_child_bounded(&mut child); return Err("读取 GPU 设备列表超时".into()); },
            Err(e) => { job.terminate(); kill_child_bounded(&mut child); return Err(format!("无法读取 GPU 设备列表：{e}")); },
        }
    };
    if !status.success() { return Err("GPU 设备枚举失败，请检查 GPU 组件和驱动".into()); }
    let bytes = rx.recv_timeout(ENUM_TIMEOUT).map_err(|_| "GPU 设备列表读取超时")?.map_err(|e| format!("读取 GPU 设备列表失败：{e}"))?;
    if bytes.len() as u64 > MAX_ENUM_BYTES { return Err("GPU 设备列表过长".into()); }
    parse_adapter_list(&bytes)
}

// Only one request can be outstanding; worker owns the pipes and may block in
// OS I/O. The caller retains the Child and kills it on timeout to unblock I/O.
enum Message { Hello, Init(Vec<f32>), Block(Vec<f32>), Stop }
enum Reply { Hello, Ready { adapter_index: u32, device_name: String }, Audio(Vec<f32>), Stopped }
struct Request { message: Message, response: SyncSender<Result<Reply, String>> }
fn read_u32(reader: &mut impl Read) -> Result<u32, String> {
    let mut data = [0u8; 4];
    reader.read_exact(&mut data).map_err(|e| format!("GPU 进程通信中断：{e}"))?;
    Ok(u32::from_le_bytes(data))
}
fn write_f32(writer: &mut impl Write, values: &[f32], scratch: &mut Vec<u8>) -> Result<(), String> {
    scratch.clear();
    scratch.reserve(values.len() * 4);
    for &value in values { scratch.extend_from_slice(&value.to_le_bytes()); }
    writer.write_all(scratch).map_err(|e| format!("GPU 音频发送失败：{e}"))?;
    writer.flush().map_err(|e| format!("GPU 音频发送失败：{e}"))
}
fn read_packet(reader: &mut impl Read, max_samples: usize) -> Result<Vec<f32>, String> {
    let count = read_u32(reader)?;
    if count == ERROR_SENTINEL {
        let size = read_u32(reader)? as usize;
        if size > MAX_ERROR { return Err("GPU 进程返回的错误信息过长".into()); }
        let mut bytes = vec![0; size];
        reader.read_exact(&mut bytes).map_err(|e| format!("GPU 进程通信中断：{e}"))?;
        return Err(format!("GPU 处理失败：{}", String::from_utf8_lossy(&bytes)));
    }
    let count = count as usize;
    if count > max_samples { return Err("GPU 进程返回的音频长度无效".into()); }
    let mut bytes = vec![0u8; count * 4];
    reader.read_exact(&mut bytes).map_err(|e| format!("GPU 进程通信中断：{e}"))?;
    let mut result = Vec::with_capacity(count);
    for chunk in bytes.chunks_exact(4) {
        let value = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        if !value.is_finite() { return Err("GPU 返回了无效音频样本".into()); }
        result.push(value);
    }
    Ok(result)
}
fn io_thread(mut input: impl Write, mut output: impl Read, rx: Receiver<Request>, block_size: usize) {
    let mut scratch = Vec::with_capacity(REFERENCE_SAMPLES * 4);
    let mut protocol_verified = false;
    while let Ok(request) = rx.recv() {
        let stop = matches!(&request.message, Message::Stop);
        let result = match request.message {
            Message::Hello => (|| {
                if protocol_verified { return Err("GPU 进程重复握手".into()); }
                if read_u32(&mut output)? != PROTOCOL_MAGIC { return Err("GPU 进程协议版本不匹配".into()); }
                protocol_verified = true;
                Ok(Reply::Hello)
            })(),
            Message::Init(reference) => (|| {
                // Never send voice until the separate, time-bounded protocol
                // handshake has succeeded.
                if !protocol_verified { return Err("GPU 进程尚未完成协议握手".into()); }
                write_f32(&mut input, &reference, &mut scratch)?;
                let response = read_packet(&mut output, 0)?;
                if !response.is_empty() { return Err("GPU 启动握手无效".into()); }
                let adapter_index = read_u32(&mut output)?;
                let size = read_u32(&mut output)? as usize;
                if size == 0 || size > 1024 { return Err("GPU 设备名称无效".into()); }
                let mut bytes = vec![0u8; size];
                output.read_exact(&mut bytes).map_err(|e| format!("GPU 进程通信中断：{e}"))?;
                let device_name = String::from_utf8(bytes).map_err(|_| "GPU 设备名称无效")?;
                if device_name.trim().is_empty() { return Err("GPU 设备名称无效".into()); }
                Ok(Reply::Ready { adapter_index, device_name })
            })(),
            Message::Block(samples) => {
                input.write_all(&(samples.len() as u32).to_le_bytes()).map_err(|e| format!("GPU 进程通信中断：{e}"))
                    .and_then(|()| write_f32(&mut input, &samples, &mut scratch))
                    .and_then(|()| read_packet(&mut output, block_size + 1024)).map(Reply::Audio)
            },
            Message::Stop => input.write_all(&0u32.to_le_bytes()).and_then(|()| input.flush()).map(|()| Reply::Stopped).map_err(|e| format!("GPU 进程停止失败：{e}")),
        };
        let failed = result.is_err();
        let _ = request.response.send(result);
        if stop || failed { break; }
    }
}

pub struct GpuExtractor {
    child: Child,
    sender: Option<SyncSender<Request>>,
    io_thread: Option<JoinHandle<()>>,
    block_size: usize,
    processed_first: bool,
    device_name: String,
    adapter_index: u32,
    // Dropped after the child and I/O thread; also kills an orphan on crash.
    job: Option<KillOnCloseJob>,
}
impl GpuExtractor {
    /// `reference` is the caller-selected 3 seconds of mono 16 kHz float PCM.
    /// The GPU worker independently checks that DirectML executed model nodes.
    pub fn new(reference: &[f32], model_dir: &Path, block_size: usize, selection: Option<(u32, u64)>) -> Result<Self, String> {
        if reference.len() != REFERENCE_SAMPLES || !reference.iter().all(|x| x.is_finite()) {
            return Err("GPU 声样必须是 3 秒有效的 16 kHz 单声道音频".into());
        }
        if !matches!(block_size, 1280 | 2560 | 3840) { return Err("GPU 模式不支持此音频块长度".into()); }
        let exe = std::env::current_exe().map_err(|e| format!("无法定位程序：{e}"))?;
        let runtime = exe.parent().ok_or("无法定位程序目录")?.join("runtime-dml");
        let worker = worker_path()?;
        let ort = runtime.join("onnxruntime.dll");
        let enroll = model_dir.join("enroll.onnx");
        let chunk = model_dir.join("chunk_int8.onnx");
        for path in [&worker, &ort, &enroll, &chunk] {
            if !path.is_file() { return Err(format!("GPU 所需文件不存在：{}", path.display())); }
        }
        // A selection carries both the original DXGI index and the stable
        // adapter identifier, so device reordering fails closed.
        let (adapter, expected_luid) = match selection {
            Some((index, luid)) if index <= i32::MAX as u32 => (index.to_string(), format!("{luid:016X}")),
            Some(_) => return Err("GPU 适配器编号无效".into()),
            None => ("-1".to_string(), "-".to_string()),
        };
        let mut child = Command::new(&worker).arg(&ort).arg(&enroll).arg(&chunk).arg("dml")
            .arg(&adapter).arg(block_size.to_string()).arg(&expected_luid)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn().map_err(|e| format!("无法启动 GPU 进程：{e}"))?;
        let job = match KillOnCloseJob::attach(&child) {
            Ok(job) => job,
            Err(e) => { kill_child_bounded(&mut child); return Err(e); }
        };
        let input = match child.stdin.take() { Some(value) => value, None => { kill_child_bounded(&mut child); return Err("无法连接 GPU 输入".into()); } };
        let output = match child.stdout.take() { Some(value) => value, None => { kill_child_bounded(&mut child); return Err("无法连接 GPU 输出".into()); } };
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker_thread = thread::spawn(move || io_thread(input, output, receiver, block_size));
        let mut extractor = Self { child, sender: Some(sender), io_thread: Some(worker_thread), block_size, processed_first: false, device_name: String::new(), adapter_index: 0, job: Some(job) };
        match extractor.exchange(Message::Hello, HANDSHAKE_TIMEOUT)? {
            Reply::Hello => (),
            _ => { extractor.stop_hard(); return Err("GPU 启动握手无效".into()); },
        }
        match extractor.exchange(Message::Init(reference.to_vec()), START_TIMEOUT)? {
            Reply::Ready { adapter_index, device_name } => {
                if selection.is_some_and(|(index, _)| index != adapter_index) { extractor.stop_hard(); return Err("GPU 设备已变更，请刷新列表后重新选择".into()); }
                extractor.adapter_index = adapter_index; extractor.device_name = device_name;
            },
            _ => { extractor.stop_hard(); return Err("GPU 启动握手无效".into()); },
        }
        Ok(extractor)
    }
    pub fn device_name(&self) -> &str { &self.device_name }
    pub fn adapter_index(&self) -> u32 { self.adapter_index }
    pub fn process_id(&self) -> u32 { self.child.id() }
    /// Child CPU time only; add the parent process CPU time for whole-chain cost.
    pub fn cpu_seconds(&self) -> Result<f64, String> {
        let (mut creation, mut exit, mut kernel, mut user) = (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
        let ok = unsafe { GetProcessTimes(self.child.as_raw_handle() as *mut c_void, &mut creation, &mut exit, &mut kernel, &mut user) };
        if ok == 0 { return Err(format!("无法读取 GPU 子进程 CPU 时间：{}", std::io::Error::last_os_error())); }
        let ticks = |t: FileTime| ((t.high as u64) << 32) | t.low as u64;
        Ok((ticks(kernel) + ticks(user)) as f64 / 10_000_000.0)
    }
    fn exchange(&mut self, message: Message, timeout: Duration) -> Result<Reply, String> {
        let timeout_message = match &message {
            Message::Hello => "GPU 进程握手超时，已停止以保护声样",
            Message::Init(_) => "GPU 模型加载超时，已停止处理",
            Message::Block(_) | Message::Stop => "GPU 处理超时，已停止以避免输出未处理音频",
        };
        let (tx, rx) = mpsc::sync_channel(1);
        if self.sender.as_ref().ok_or("GPU 进程已停止")?.try_send(Request { message, response: tx }).is_err() {
            self.stop_hard();
            return Err("GPU 进程已退出".into());
        }
        match rx.recv_timeout(timeout) {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(error)) => { self.stop_hard(); Err(error) },
            Err(mpsc::RecvTimeoutError::Timeout) => { self.stop_hard(); Err(timeout_message.into()) },
            Err(mpsc::RecvTimeoutError::Disconnected) => { self.stop_hard(); Err("GPU 进程异常退出".into()) },
        }
    }
    pub fn feed(&mut self, input: &[f32]) -> Result<Vec<f32>, String> {
        if input.len() != self.block_size || !input.iter().all(|x| x.is_finite()) { return Err("GPU 输入音频块无效".into()); }
        match self.exchange(Message::Block(input.to_vec()), BLOCK_TIMEOUT)? {
            Reply::Audio(output) => {
                if !valid_output_length(self.block_size, self.processed_first, output.len()) {
                    self.stop_hard();
                    return Err("GPU 返回的音频长度与流式模型不符，已停止处理".into());
                }
                self.processed_first = true;
                Ok(output)
            },
            _ => { self.stop_hard(); Err("GPU 进程返回了无效响应".into()) },
        }
    }
    fn stop_hard(&mut self) {
        // Terminate the whole job before waiting for a pipe reader. If the
        // process or an inherited handle misbehaves, never block this thread
        // forever waiting for it or the I/O thread.
        if let Some(job) = self.job.as_ref() { job.terminate(); }
        self.job.take(); // KILL_ON_JOB_CLOSE is a second termination path.
        let _ = self.child.kill();
        self.sender.take();
        let until = Instant::now() + STOP_TIMEOUT;
        while Instant::now() < until {
            match self.child.try_wait() { Ok(Some(_)) | Err(_) => break, Ok(None) => thread::sleep(Duration::from_millis(10)) }
        }
        if let Some(thread) = self.io_thread.take() { if thread.is_finished() { let _ = thread.join(); } }
    }
}

fn valid_output_length(block_size: usize, processed_first: bool, output_len: usize) -> bool {
    if !matches!(block_size, 1280 | 2560 | 3840) { return false; }
    let expected = if processed_first { block_size } else { block_size - 384 };
    output_len == expected
}

#[cfg(test)]
mod tests {
    use super::{parse_adapter_list, valid_output_length};
    #[test]
    fn adapter_list_accepts_distinct_hardware_and_duplicate_names() {
        let list = b"GPU2-LIST\t1\n0\t0000000000000001\t000010DE\t00001234\t4E5649444941\n1\t0000000000000002\t00001002\t00005678\t414D44\n2\t0000000000000003\t00008086\t00000001\t496E74656C\n3\t0000000000000004\t000010DE\t00001235\t4E5649444941\n";
        let adapters = parse_adapter_list(list).unwrap();
        assert_eq!(adapters.len(), 4);
        assert_eq!(adapters[0], (0, 1, "NVIDIA".into()));
        assert_eq!(adapters[1], (1, 2, "AMD".into()));
        assert_eq!(adapters[2], (2, 3, "Intel".into()));
        assert_eq!(adapters[3], (3, 4, "NVIDIA".into()));
        assert!(parse_adapter_list(b"GPU2-LIST\t1\n").unwrap().is_empty());
    }
    #[test]
    fn adapter_list_rejects_invalid_or_ambiguous_identifiers() {
        assert!(parse_adapter_list(b"GPU1-LIST\t1\n").is_err());
        assert!(parse_adapter_list(b"GPU2-LIST\t1\n0\t0000000000000001\t000010DE\t00001234\tZZ\n").is_err());
        assert!(parse_adapter_list(b"GPU2-LIST\t1\n0\t0000000000000001\t000010DE\t00001234\t41\n1\t0000000000000001\t00001002\t00005678\t42\n").is_err());
    }
    #[test]
    fn fixed_stream_rejects_missing_or_incorrect_output() {
        for block in [1280, 2560, 3840] {
            assert!(valid_output_length(block, false, block - 384));
            assert!(valid_output_length(block, true, block));
            for count in [0, block - 385, block - 383, block + 1] {
                assert!(!valid_output_length(block, false, count));
                assert!(!valid_output_length(block, true, count));
            }
        }
        assert!(!valid_output_length(0, false, 0));
    }
}
impl Drop for GpuExtractor {
    fn drop(&mut self) {
        if self.sender.is_none() { return; }
        let (tx, rx) = mpsc::sync_channel(1);
        let sent = self.sender.as_ref().is_some_and(|sender| sender.try_send(Request { message: Message::Stop, response: tx }).is_ok());
        if sent { let _ = rx.recv_timeout(STOP_TIMEOUT); }
        let until = Instant::now() + STOP_TIMEOUT;
        while Instant::now() < until {
            match self.child.try_wait() {
                Ok(Some(_)) => { self.sender.take(); if let Some(thread) = self.io_thread.take() { if thread.is_finished() { let _ = thread.join(); } } return; },
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        self.stop_hard();
    }
}
