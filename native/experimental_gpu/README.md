# DirectML 声纹提取工作进程

本目录实现正式程序可选的 DirectML 后端。程序默认使用 CPU；用户在界面启用显卡加速后，使用与 CPU 路径相同的 VoxLock V1 模型（`models/enroll.onnx`、`chunk_int8.onnx`），将声纹提取放到独立的 Windows 子进程；主进程使用 CPU 版 sherpa-onnx 做后续降噪。独立进程避免 sherpa 依赖的 ONNX Runtime 1.28.2 与 DirectML ONNX Runtime 1.24.4 的同名 DLL 冲突。

`worker.cpp` 通过 DXGI 枚举硬件适配器，不按 AMD、NVIDIA、Intel 等厂商限制；会排除 DXGI 标记为软件的适配器。显式选卡时同时验证原始 DXGI 序号与 LUID，不匹配就要求刷新；自动模式优先选择专用显存最大的硬件卡，平局选原始序号较小者。所选卡若不支持 DirectML 或不能实际执行模型计算则明确失败，不会悄悄切到另一张卡或 CPU。启动时用生成的非个人测试信号分别运行首块与稳态的 session 并读取 ORT profile；只有两个 session 都实际包含 DirectML 非数据拷贝计算节点时才握手成功。模型中的部分算子仍可能运行在 CPU。诊断 profile 不含用户声样；正常和错误退出会清理 profile。

`--list-adapters` 可只读列出设备；枚举格式、启动参数及 GPU2 握手见 [PROTOCOL.md](PROTOCOL.md)。DXGI LUID 用于校验当前设备列表，不能作为跨重启稳定标识。GPU2 不兼容旧 GPU1 worker，必须同时更新程序与运行库。当前机器仅在 AMD Radeon RX 6800 上实测；通用代码不等于已在 NVIDIA 或其他型号、驱动上完成实机验证。

固定块输入为 16 kHz 单声道 f32，支持 1280、2560、3840 样本（80、160、240 ms）。首块分别使用固定模型长度 1536、2816、4096；稳态使用 1664、2944、4224；参考声样恰好 48000 个样本。块长错误、非有限输入/输出、超时、异常退出均报告错误，绝不绕过模型输出原声。子进程通过长度受限的二进制管道工作；`native/src/gpu_worker.rs` 将它放进父进程退出即终止的 Windows 作业对象，限制启动/每块处理时间。

## 构建运行库

需要 Windows x64 的 MinGW-w64 C++ 编译器、Windows SDK/DXGI 头文件和库。用户自行安装可信编译器后执行：

```powershell
powershell -ExecutionPolicy Bypass -File native/setup-gpu.ps1 -Cxx <g++.exe路径>
```

脚本下载并校验微软 `Microsoft.ML.OnnxRuntime.DirectML` 1.24.4 官方包的 SHA-256，验证 DLL 后构建 worker，安装到 `native/target/release/runtime-dml`；不会替换 CPU 版运行库，也不修改 Windows 自带的 DirectML.dll。也可以 `-Package <已经下载的nupkg>` 离线构建。系统需要 Windows 的 DirectML 运行库、所选显卡的兼容驱动，以及 Microsoft Visual C++ x64 运行库。

## 局限

RX 6800 上已完成 GPU/CPU 合成语音处理链对照；性能与音质数据及测试条件见 [DirectML 测试报告](../benchmarks/results/directml-rust-20261116/README.md)。这些离线节拍测试不包含真实声卡往返延迟，也不能代表其他显卡、驱动或真实通话环境。量化 LSTM 等部分节点可能运行在 CPU；游戏竞争和驱动服务的 CPU 开销没有由程序进程计时包含。实际效果应结合所用设备、声音和系统负载判断。
