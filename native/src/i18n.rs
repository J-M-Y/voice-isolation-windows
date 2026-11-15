//! Presentation-time translations for controller status and diagnostic text.
//!
//! The controller retains the original message, allowing an already-visible
//! status to change language immediately. Recognized wrappers are translated
//! recursively; variable device names, paths, and OS errors are preserved.

fn gpu_name(name: &str) -> String {
    if let Some((label, id)) = name.rsplit_once("（DXGI 设备 ") {
        if let Some(id) = id.strip_suffix('）') {
            if id.parse::<u32>().is_ok() {
                return format!("{label} (DXGI device {id})");
            }
        }
    }
    name.to_owned()
}

/// Render a status without changing the stored original. Unknown diagnostic
/// details are retained verbatim so that troubleshooting information is never
/// silently discarded or incorrectly presented as a different error.
pub fn render_status(message: &str, english: bool) -> String {
    if !english { return message.to_owned(); }

    if let Some(rest) = message.strip_prefix("当前显卡：") {
        if let Some((name, detail)) = rest.split_once("。 音频缓冲异常：") {
            let translated = render_status(&format!("音频缓冲异常：{detail}"), true);
            return format!("Current GPU: {}. {translated}", gpu_name(name));
        }
    }
    if let Some(rest) = message.strip_prefix("音频缓冲异常：输入丢块 ") {
        if let Some((input, rest)) = rest.split_once("，输出丢块 ") {
            if let Some((output, rest)) = rest.split_once("，输出缺样 ") {
                if let Some((callbacks, rest)) = rest.split_once(" 次（累计约 ") {
                    if let Some(ms) = rest.strip_suffix(" 毫秒，含恢复缓冲）。若持续增加，请减轻系统负载。") {
                        return format!("Audio buffering: {input} input blocks dropped, {output} output blocks dropped, {callbacks} output underruns ({ms} ms total, including rebuffering). Reduce system load if the counts keep rising.");
                    }
                }
            }
        }
    }
    if let Some(name) = message.strip_prefix("正在使用 ").and_then(|s| s.strip_suffix(" 加速声纹提取；降噪使用 CPU。")) {
        return format!("Using {} for voice extraction; noise reduction uses the CPU.", gpu_name(name));
    }
    if let Some(path) = message.strip_prefix("GPU 组件不存在：").and_then(|s| s.strip_suffix("；请先安装 GPU 组件")) {
        return format!("GPU component is missing: {path}; install the GPU components first");
    }
    if let Some(code) = message.strip_prefix("无法创建单实例保护（Windows 错误 ").and_then(|s| s.strip_suffix('）')) {
        return format!("Could not create the single-instance lock (Windows error {code})");
    }

    let exact = match message {
        "正在读取音频设备…" => "Loading audio devices…",
        "程序已在运行。请从系统托盘打开窗口。" => "The application is already running. Open it from the system tray.",
        "后台处理线程已退出，请重新启动程序。" => "The background controller has exited. Restart the application.",
        "设备列表已刷新。" => "Device list refreshed.",
        "试听已准备好。" => "Preview is ready.",
        "声样录制完成。" => "Voice sample recorded.",
        "正在使用 CPU 处理音频。" => "Processing audio on the CPU.",
        "播放完成。" => "Playback finished.",
        "后台任务异常退出，请重试。" => "A background task stopped unexpectedly. Try again.",
        "处理已停止。" => "Processing stopped.",
        "请先录制试听。" => "Record a preview first.",
        "正在验证显卡并加载模型，可能需要十几秒…" => "Checking the GPU and loading models; this may take several seconds…",
        "正在加载模型…" => "Loading models…",
        "正在停止…" => "Stopping…",
        "正在刷新设备…" => "Refreshing devices…",
        "录音中，请连续说话 12 秒…" => "Recording: speak continuously for 12 seconds…",
        "正在录制并处理试听…" => "Recording and processing preview…",
        "正在播放原声…" => "Playing original audio…",
        "正在播放处理结果…" => "Playing processed audio…",
        "声样的声道数或采样率无效" => "The voice sample has an invalid channel count or sample rate",
        "不支持这种 WAV 格式，请录制新的声样" => "Unsupported WAV format; record a new voice sample",
        "声样太短，请至少录制 3 秒" => "The voice sample is too short; record at least 3 seconds",
        "声样至少需要 3 秒" => "The voice sample must be at least 3 seconds long",
        "声样至少需要 3 秒有效音频" => "The voice sample must contain at least 3 seconds of valid audio",
        "声样重采样后不足 3 秒，请重新录制" => "The resampled voice sample is shorter than 3 seconds; record it again",
        "声样音量过低或无效，请重新录制" => "The voice sample is too quiet or invalid; record it again",
        "声样未成功录入，已阻止原始声音通过，请重新录制" => "Voice enrollment failed; unprocessed audio was blocked. Record the voice sample again",
        "声样录入失败，请重新录制" => "Voice enrollment failed; record the sample again",
        "设备已变更或存在同名设备，请刷新列表并排除同名设备" => "The device changed or duplicate device names exist; refresh the list and remove duplicates",
        "音频设备已不存在，请刷新设备列表" => "The audio device is no longer available; refresh the device list",
        "设备顺序已变更，请刷新设备列表并重新选择" => "The device order changed; refresh the list and select again",
        "未知设备" => "Unknown device",
        "无效的处理延迟设置" => "Invalid processing latency setting",
        "输入设备已断开" => "Input device disconnected",
        "音频线程意外退出" => "Audio thread stopped unexpectedly",
        "模型输出无效" => "The model returned invalid output",
        "模型返回了无效音频" => "The model returned invalid audio",
        "模型返回了无效音频样本" => "The model returned invalid audio samples",
        "模型持续没有输出，请检查模型和运行库" => "The model is not producing output; check the models and runtime",
        "创建语音模型失败，请检查模型和运行库版本" => "Failed to create the voice model; check the model and runtime versions",
        "模型采样率不是 16000 Hz" => "The model sample rate is not 16000 Hz",
        "VoxLock 模型的帧长不匹配" => "The VoxLock model frame size does not match",
        "模型路径包含无效字符" => "The model path contains invalid characters",
        "音频块过大" => "Audio block is too large",
        "音频块长度必须是 3 的倍数" => "Audio block length must be divisible by 3",
        "音频数据无效" => "Invalid audio data",
        "录音时长必须在 1 到 60 秒之间" => "Recording duration must be between 1 and 60 seconds",
        "录音数据丢失，请重试并关闭占用过高的程序" => "Recording data was lost; retry after closing resource-intensive programs",
        "录音超时，请检查麦克风" => "Recording timed out; check the microphone",
        "录音设备已断开" => "Recording device disconnected",
        "录音音量过低或无效，请检查所选麦克风后重新录制" => "Recording is too quiet or invalid; check the microphone and record again",
        "播放超时，请检查输出设备" => "Playback timed out; check the output device",
        "播放设备已断开" => "Playback device disconnected",
        "运行库文件过大" => "Runtime file is too large",
        "无法初始化运行库校验" => "Could not initialize runtime verification",
        "无法校验运行库" => "Could not verify the runtime",
        "无法创建临时声样文件" => "Could not create the temporary voice sample",
        "运行库缺少声样接口，且不是已验证的官方 2.0.1 版本" => "The runtime has no voice sample interface and is not the verified official 2.0.1 release",
        "sherpa-onnx 运行库版本不匹配，需要 2.0.1" => "The sherpa-onnx runtime version is incompatible; version 2.0.1 is required",
        "无法定位程序目录" => "Could not locate the application directory",
        "GPU 列表编码无效" => "Invalid GPU list encoding",
        "GPU 设备列表协议不兼容，请重新安装 GPU 组件" => "Incompatible GPU list protocol; reinstall the GPU components",
        "GPU 设备列表过长" => "GPU list is too long",
        "GPU 设备列表格式无效" => "Invalid GPU list format",
        "GPU 设备编号无效" => "Invalid GPU device index",
        "GPU 设备标识无效" => "Invalid GPU device identifier",
        "GPU 厂商标识无效" => "Invalid GPU vendor identifier",
        "GPU 型号标识无效" => "Invalid GPU model identifier",
        "GPU 名称编码无效" => "Invalid GPU name encoding",
        "GPU 设备列表包含无效或重复标识" => "GPU list contains invalid or duplicate identifiers",
        "无法读取 GPU 设备列表" => "Could not read the GPU device list",
        "读取 GPU 设备列表超时" => "Timed out while reading the GPU device list",
        "GPU 设备列表读取超时" => "Timed out while reading the GPU device list",
        "GPU 设备枚举失败，请检查 GPU 组件和驱动" => "GPU enumeration failed; check GPU components and drivers",
        "GPU 进程返回的错误信息过长" => "GPU worker error message is too long",
        "GPU 进程返回的音频长度无效" => "GPU worker returned an invalid audio length",
        "GPU 返回了无效音频样本" => "GPU worker returned invalid audio samples",
        "GPU 进程重复握手" => "GPU worker repeated its handshake",
        "GPU 进程协议版本不匹配" => "GPU worker protocol version mismatch",
        "GPU 进程尚未完成协议握手" => "GPU worker has not completed its protocol handshake",
        "GPU 启动握手无效" => "Invalid GPU worker startup handshake",
        "GPU 设备名称无效" => "Invalid GPU device name",
        "GPU 声样必须是 3 秒有效的 16 kHz 单声道音频" => "GPU enrollment requires 3 seconds of valid 16 kHz mono audio",
        "GPU 模式不支持此音频块长度" => "This audio block size is not supported in GPU mode",
        "GPU 适配器编号无效" => "Invalid GPU adapter index",
        "无法连接 GPU 输入" => "Could not connect to the GPU worker input",
        "无法连接 GPU 输出" => "Could not connect to the GPU worker output",
        "GPU 设备已变更，请刷新列表后重新选择" => "The GPU device changed; refresh the list and select it again",
        "GPU 进程握手超时，已停止以保护声样" => "GPU handshake timed out; stopped to protect the voice sample",
        "GPU 模型加载超时，已停止处理" => "GPU model loading timed out; processing stopped",
        "GPU 处理超时，已停止以避免输出未处理音频" => "GPU processing timed out; stopped to prevent unprocessed audio output",
        "GPU 进程已停止" => "GPU worker has stopped",
        "GPU 进程已退出" => "GPU worker has exited",
        "GPU 进程异常退出" => "GPU worker exited unexpectedly",
        "GPU 输入音频块无效" => "Invalid GPU input audio block",
        "GPU 返回的音频长度与流式模型不符，已停止处理" => "GPU audio length does not match the streaming model; processing stopped",
        "GPU 进程返回了无效响应" => "GPU worker returned an invalid response",
        _ => "",
    };
    if !exact.is_empty() { return exact.to_owned(); }

    const PREFIXES: &[(&str, &str)] = &[
        ("录制失败：", "Recording failed: "), ("启动失败：", "Start failed: "),
        ("试听失败：", "Preview failed: "), ("播放失败：", "Playback failed: "),
        ("设备读取失败：", "Failed to read devices: "), ("音频处理已停止：", "Audio processing stopped: "),
        ("处理模式已切换，但无法保存设置：", "Processing mode changed, but settings could not be saved: "),
        ("界面语言已切换，但无法保存设置：", "Display language changed, but settings could not be saved: "),
        ("显卡列表不可用：", "GPU list unavailable: "), ("无法读取声样 ", "Could not read voice sample "),
        ("无法启动界面：", "Could not start the interface: "),
        ("无法枚举音频设备: ", "Could not enumerate audio devices: "),
        ("无法读取输入格式: ", "Could not read input format: "), ("无法读取输出格式: ", "Could not read output format: "),
        ("输入设备错误: ", "Input device error: "), ("输出设备错误: ", "Output device error: "),
        ("不支持的输入采样格式: ", "Unsupported input sample format: "), ("不支持的输出采样格式: ", "Unsupported output sample format: "),
        ("无法打开输入设备: ", "Could not open input device: "), ("无法打开输出设备: ", "Could not open output device: "),
        ("无法启动输入: ", "Could not start input: "), ("无法启动输出: ", "Could not start output: "),
        ("无法启动录音: ", "Could not start recording: "), ("无法启动播放: ", "Could not start playback: "),
        ("无法创建临时录音: ", "Could not create temporary recording: "), ("无法保存录音: ", "Could not save recording: "),
        ("无法定位程序目录：", "Could not locate the application directory: "), ("无法定位程序：", "Could not locate the application: "),
        ("无法打开 sherpa-onnx 运行库：", "Could not open the sherpa-onnx runtime: "),
        ("无法加载 sherpa-onnx-c-api.dll（需 2.0.1 及配套 DLL）：", "Could not load sherpa-onnx-c-api.dll (version 2.0.1 and matching DLLs required): "),
        ("sherpa-onnx 版本不兼容，缺少 ", "Incompatible sherpa-onnx version; missing "),
        ("无法校验运行库：", "Could not verify the runtime: "), ("无法准备临时声样：", "Could not prepare temporary voice sample: "),
        ("缺少模型文件：", "Missing model file: "),
        ("无法创建 GPU 进程保护：", "Could not create GPU worker protection: "),
        ("无法设置 GPU 进程保护：", "Could not configure GPU worker protection: "),
        ("无法关联 GPU 进程保护：", "Could not attach GPU worker protection: "),
        ("GPU 组件不存在：", "GPU component is missing: "),
        ("无法读取 GPU 设备：", "Could not read GPU devices: "),
        ("无法读取 GPU 设备列表：", "Could not read GPU device list: "),
        ("读取 GPU 设备列表失败：", "Failed to read GPU device list: "),
        ("GPU 进程通信中断：", "GPU worker communication interrupted: "),
        ("GPU 音频发送失败：", "Failed to send audio to GPU worker: "),
        ("GPU 处理失败：", "GPU processing failed: "),
        ("GPU 进程停止失败：", "Failed to stop GPU worker: "),
        ("GPU 所需文件不存在：", "Required GPU file is missing: "),
        ("无法启动 GPU 进程：", "Could not start GPU worker: "),
        ("无法读取 GPU 子进程 CPU 时间：", "Could not read GPU worker CPU time: "),
    ];
    for &(from, to) in PREFIXES {
        if let Some(rest) = message.strip_prefix(from) {
            return format!("{to}{}", render_status(rest, true));
        }
    }
    message.to_owned()
}

#[cfg(test)]
mod tests {
    use super::render_status;

    #[test]
    fn translates_nested_errors_and_preserves_diagnostics() {
        let input = "启动失败：无法打开输入设备: device missing";
        assert_eq!(render_status(input, false), input);
        assert_eq!(render_status(input, true), "Start failed: Could not open input device: device missing");
        assert_eq!(render_status("启动失败：GPU 处理失败：invalid adapter", true), "Start failed: GPU processing failed: invalid adapter");
        assert_eq!(render_status("启动失败：未来的新错误", true), "Start failed: 未来的新错误");
    }

    #[test]
    fn translates_live_gpu_and_counters_without_changing_name_or_values() {
        assert_eq!(render_status("正在使用 AMD Radeon RX 6800（DXGI 设备 1） 加速声纹提取；降噪使用 CPU。", true),
            "Using AMD Radeon RX 6800 (DXGI device 1) for voice extraction; noise reduction uses the CPU.");
        let raw = "当前显卡：AMD Radeon RX 6800（DXGI 设备 0）。 音频缓冲异常：输入丢块 1，输出丢块 2，输出缺样 3 次（累计约 40.5 毫秒，含恢复缓冲）。若持续增加，请减轻系统负载。";
        assert_eq!(render_status(raw, true), "Current GPU: AMD Radeon RX 6800 (DXGI device 0). Audio buffering: 1 input blocks dropped, 2 output blocks dropped, 3 output underruns (40.5 ms total, including rebuffering). Reduce system load if the counts keep rising.");
    }
}
