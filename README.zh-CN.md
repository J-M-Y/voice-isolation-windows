# 指定人声提取 · Windows

[English](README.md) · [简体中文](README.zh-CN.md) · [Downloads / 下载](https://github.com/J-M-Y/voice-isolation-windows/releases/latest) · [Changelog / 版本记录](CHANGELOG.md)

打游戏语音时，即使旁边播放音乐、刷短视频或有其他背景声，也尽量让队友听清你的声音、少听到背景声。录入自己的声样后，程序在 Windows 本地处理麦克风音频，再输出到虚拟声卡；Rust 原生程序运行时无需 Python。

## 功能

- 录制或导入 WAV 声样，提取目标说话人，并提供普通和加强背景音乐过滤。
- 软件界面可在简体中文与 English 之间切换，并记住所选语言。
- 选择输入、输出和试听设备，录制五秒进行处理前后对比。
- 选择 **低延迟**、**省 CPU**或**更省 CPU**处理模式；较大的处理块减少模型调用次数，也会增加等待时间。
- 选择 CPU 或通过 Windows DirectML 使用兼容的 AMD、NVIDIA、Intel 显卡；支持自动或手动选卡，无需 CUDA。模型的部分算子和降噪仍在 CPU 执行。
- 支持托盘运行，并显示丢块、输出缺样次数及累计缺样时长。

程序处理的是麦克风输入，不会消除你自己扬声器中的音乐或视频声音。声音效果、CPU 占用和延迟会随设备与环境变化；无法保证完全去除背景声，也不包含专门的声学回声消除。DirectML 代码支持多个显卡厂商，当前已在 AMD Radeon RX 6800 上实测，NVIDIA 和其他型号尚未完成实机测试。

## 下载安装

从 [GitHub Releases](https://github.com/J-M-Y/voice-isolation-windows/releases/latest) 下载 Windows x64 安装包或便携压缩包。便携版解压到一个文件夹后运行 `VoiceIsolation.exe`，请保留压缩包内的模型与运行库目录。安装包与便携版均不需要 Python。若系统缺少运行库，请安装微软官方 [Visual C++ 2015–2022 x64 可再发行组件](https://learn.microsoft.com/zh-cn/cpp/windows/latest-supported-vc-redist)。

程序本身不安装虚拟声卡。需自行安装 [VB-CABLE](https://vb-audio.com/Cable/) 或其他兼容的虚拟声卡；安装第三方驱动可能需要管理员权限及重启。VB-CABLE 是 VB-Audio 的独立软件，使用及再分发以其[官方授权](https://vb-audio.com/Services/licensing.htm)为准。

使用显卡加速时，需要 Windows 10 版本 1903 或更新版本、支持 DirectX 12 / DirectML 的显卡及合适的驱动；具体型号对模型和运算符的支持可能不同。若显卡初始化或实际 GPU 计算验证失败，程序会提示错误，可取消显卡加速后使用 CPU。[DirectML 的系统要求](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html)

## 使用方法

1. 在窗口顶部的语言选择器中选择 **简体中文**或 **English**。首次启动按 Windows 显示语言选择，切换立即生效并保存。
2. 选择实体麦克风，点击 **录入声音（12 秒）**，在安静环境正常、连续说话；也可以导入已有 WAV 声样。
3. 将输出设备设为虚拟声卡的播放端，例如 **CABLE Input**，然后点击 **开始处理**。
4. 在游戏或语音软件中，将麦克风选择为虚拟声卡的录音端，例如 **CABLE Output**；耳机输出保持原来的设备。
5. 停止处理后，可选择实体耳机或扬声器录制并试听原声和处理后的效果。

关闭或最小化窗口后程序留在系统托盘。双击托盘图标显示窗口，右键选择 **退出软件**，或在主窗口点击 **退出软件**，会停止处理并退出。若看不到图标，请展开任务栏右侧的隐藏图标区域。旧版仍在运行时，请先退出旧版再启动新版。

三个处理模式分别使用 80、160、240 毫秒音频块，并分别增加约 20、40、80 毫秒播放预缓冲以抵御短时波动。实际端到端延迟还包括模型、声卡、系统和网络等开销。若界面中缺样或丢块计数持续增长，声音可能出现停顿；可尝试减轻系统负载或调整处理模式。

默认声样位于 `%LOCALAPPDATA%\VoiceIsolation\我的声样.wav`，也可以选择其他 WAV 文件。正常处理不上传音频。CPU 运行库加载声样时会短暂创建临时 WAV 并在载入后删除；强制结束进程时临时文件可能残留。声样不会随源码或发布包分发。

## 从源码构建

需要 Windows x64、Rust 和 Windows C/C++ 构建工具。在仓库根目录运行：

```powershell
powershell -ExecutionPolicy Bypass -File native/setup.ps1
cargo build --release --locked --manifest-path native/Cargo.toml --bins
```

`setup.ps1` 下载并校验固定版本的模型及 CPU 运行库。显卡模式还需使用 MinGW-w64 `g++.exe` 执行 `native/setup-gpu.ps1 -Cxx <g++.exe路径>`，准备独立的 DirectML 运行库与工作进程。源码构建、测试、模型来源和性能结果详见 [Rust 构建说明](https://github.com/J-M-Y/voice-isolation-windows/blob/main/native/README.md)。

仓库保留旧 Python 实现；它与当前 Rust 版本使用不同版本的 VoxLock 模型，不能同时运行，也不能直接互换模型文件。

## 第三方组件

本项目依赖 [VoxLock](https://github.com/arbazsiddiqui/voxlock)、[GTCRN](https://github.com/Xiaobin-Rong/gtcrn)、[DPDFNet2](https://huggingface.co/Ceva-IP/DPDFNet)、[sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)、ONNX Runtime 及其他组件；各组件的许可和版权归其作者所有。发布包中的第三方许可文件列出所含组件的授权信息。虚拟声卡驱动不属于本项目。
