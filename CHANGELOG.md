# 版本记录 / Changelog

## v2.0.0

### 中文

- 发布 Rust 原生 Windows x64 版本，运行时无需 Python；保留声样录制、指定人声提取、降噪、设备选择、试听和托盘操作。
- 提供 CPU 与基于 DirectML 的通用显卡处理，可自动选卡或手动选择兼容的 AMD、NVIDIA、Intel 等显卡；模型部分算子及降噪仍使用 CPU。
- 增加 80、160、240 毫秒处理模式，以及对应的约 20、40、80 毫秒播放预缓冲，以平衡处理开销、延迟与短时抖动。
- 更新界面，显示输入/输出丢块及输出缺样的次数与累计时长；修复隐藏窗口后托盘事件可能无法唤醒界面的问题，并为退出时的后台等待设置上限。
- 提供安装包与便携压缩包；虚拟声卡驱动需单独安装。

验证范围：DirectML 已在 AMD Radeon RX 6800 上测试，NVIDIA 等显卡尚未实机验证；音质和性能随环境、设备和负载变化，不保证完全消除噪声。采集块长及播放缓冲不是端到端总延迟。详细测试条件见 [Rust 构建说明](native/README.md)及其中的基准报告。

### English

- Release the native Rust Windows x64 application; Python is not needed at runtime. Voice enrollment, speaker extraction, denoising, device selection, preview, and tray operation are included.
- Offer CPU processing and vendor-neutral DirectML acceleration with automatic or manual selection of compatible AMD, NVIDIA, Intel, and other GPUs. Some model operators and noise reduction still run on the CPU.
- Add 80, 160, and 240 ms processing modes, with approximately 20, 40, and 80 ms of playback pre-buffering respectively to trade latency for resilience to short scheduling fluctuations.
- Update the interface to show dropped blocks, underrun counts, and cumulative missing-audio duration. Fix tray events that could fail to wake a hidden window and bound the wait for background shutdown.
- Provide an installer and portable ZIP; install a virtual audio driver separately.

Validation: DirectML was tested on an AMD Radeon RX 6800; NVIDIA and other GPU models have not been tested on physical hardware. Sound quality and performance vary with the environment, hardware, and load; complete noise removal is not guaranteed. Block size and playback reserve do not represent total end-to-end latency. See the [Rust build notes](native/README.md) for test conditions and benchmark reports.
