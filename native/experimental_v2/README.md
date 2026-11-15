# VoxLock V2 原生后端实验

本目录为独立的离线实验，**未接入程序，不能用于实时音频或替换默认后端**。它直接使用原 Python 版的 `onnx/v2` 声样与流式 ONNX 模型，并调用既有的 ONNX Runtime C API。保留 512 点窗口、128 点帧移、初始镜像填充、Hann 平方包络重叠相加和 LSTM 状态。它不做音频设备 I/O、重采样、降噪或末尾 flush。

构建需 Windows x64 的 C++ 编译器和 ONNX Runtime C API v24 头文件（`onnxruntime_c_api.h`、`onnxruntime_ep_c_api.h`）。例如：

```powershell
powershell -File native/experimental_v2/build.ps1 -Cxx <g++.exe路径> -OrtInclude <ORT头文件目录>
```

`compare.py` 可对同一段明确传入的 48 kHz、16 位单声道测试 WAV 进行原 Python 和实验 DLL 的分离器对照，分别在独立进程运行 `python` 和 `native` 子命令，再用 `compare` 子命令比对输出。两边采用相同的测试重采样和最响 3 秒声样；默认计时仅包括预热后的 30 秒分离器，可用 `--seconds` 指定其他时长，不包括降噪与音频 I/O。请使用公开或合成音频，不要使用未授权的个人声样。`--help` 显示参数。

实验尚需覆盖参考音频预处理与原版 soxr 的一致性、不同块边界、长时间运行、错误恢复和竞争说话人的效果；单次测试不能替代完整应用与真实场景验证。

本机一次模型级初测：i7-12700KF，系统合成语音，先预热 2 秒，再按 80 ms 块处理 30 秒；Python 原版 V2（ONNX Runtime 1.31.1）消耗进程 CPU 12.141 秒，实验 DLL（ONNX Runtime 1.28.2）消耗 12.719 秒。两者输出长度均为 511,616 个 16 kHz 样本，输出差的 RMS 为 0.00000736，相对于 Python 输出 RMS 的 0.0186%。该单次测试仅覆盖分离器，不包含降噪、设备和界面，不能证明实时整机负载或不同环境音质；本次没有发现模型 CPU 降低。
