# 2026-11-16 DirectML 离线实况节拍测试

这是本机一次探索性测试，不是正式产品的设备往返延迟或整机功耗基准。CPU 为 Intel i7-12700KF，GPU 为 AMD Radeon RX 6800。测试时已有语音程序和 Chrome 等进程同时运行；CPU/GPU 顺序测试，各一轮，不能排除负载和时钟变化。

## 方法

- 使用 SAPI Zira 目标声和 Huihui 干扰声的合成混音；两者整段 RMS 相等。16 秒片段以 100 ms 线性交叉淡化循环，提供 2.4 秒预热及 30.24 秒测量。测试未读取个人声样、麦克风或游戏音频。
- 使用相同 VoxLock V1 INT8 模型、相同 GTCRN 降噪模型、240 ms 音频块、同一 3 秒 16 kHz PCM16 声样。两路均由正式 Rust `realtime_compare` 处理；GPU 路径额外使用独立 checked worker 和 DirectML 版 ONNX Runtime。GPU worker 握手确认 `AMD Radeon RX 6800`，启动时 profile 检查有非传输型 DirectML 模型节点；部分算子仍在 CPU 执行。
- CPU 使用单进程的进程 CPU 时间；GPU 使用主进程与 worker 的进程 CPU 时间之和。分别除以 30.24 秒音频时长，得到单逻辑核等效负载。不包括 Windows Audio、设备 I/O、UI、其他进程、GPU 采样器或驱动服务的 CPU 成本。GPU 运行另有性能计数器采样进程，它本身可能与被测进程竞争资源。
- 每块处理耗时为主处理进程调用开始至返回的墙钟时间；逾期指模型在下一块采集完毕时仍未返回，不等同实际设备丢帧。无丢包 packet 等待模拟包含一个采集块的等待、处理与不丢块排队；**不含离线模型信号偏移、实际声卡、Windows Audio、游戏和网络延迟**。

## 30.24 秒、3 秒 16 kHz 声样、普通降噪

| 指标 | Rust CPU | Rust + DirectML worker |
| --- | ---: | ---: |
| 主进程与 worker CPU 时间 | 15.7188 s | 14.1094 s |
| 单逻辑核等效负载 | 51.98% | 46.66% |
| 处理每块 p50 / p95 / p99 | 121.49 / 161.00 / 182.78 ms | 119.18 / 177.31 / 203.18 ms |
| 晚于下一块采集期限 | 0 / 126 | 0 / 126 |
| 无丢包 packet 等待模拟 p50 / p95 | 422.99 / 422.99 ms | 447.42 / 447.42 ms |
| 初始化 | 1.72 s | 8.37 s |

两者 30.24 秒输出经离线声源时间对齐，CPU/GPU 相关 0.9999744，差值 RMS 为 CPU 输出 RMS 的约 0.719%；输出和目标声的相关为 0.9723 / 0.9717，与干扰声为 0.05992 / 0.05983。两者离线信号偏移均 50 ms。相关性只反映这一组合成样本，不能保证真实话筒和多人环境同样有效。

RX 6800 上 GPU 工作进程的 3D 引擎计数器，在约 31.2 秒的近似窗口中有 9 个有效采样，均值 5.79%、峰值 6.29%；采样稀疏。它仅是该进程在该引擎的占用，不是整张显卡总占用，未包含其他进程和驱动服务。

## 48 kHz / 44.1 kHz 声样预处理短测

CPU 和 GPU 路径对原 6 秒 SAPI 声样分别采用各自实际预处理，各跑一轮：2.4 秒预热 + 6 秒双人混音、240 ms、普通 GTCRN。GPU 对声样使用本应用 `gpu_reference` 窗函数重采样并选择最响的 3 秒；CPU 通过 sherpa 内置重采样与选段。每组仅 25 块，不能推断稳定性能。

| 声样 | CPU / GPU 单核等效 | 处理 p95 CPU / GPU | packet 等待模拟 p50/p95 CPU | packet 等待模拟 p50/p95 GPU | CPU/GPU 逾期 |
| --- | --- | --- | --- | --- | --- |
| 48 kHz | 52.60% / 42.71% | 153.35 / 167.41 ms | 393.86 / 405.34 ms | 433.21 / 433.21 ms | 0 / 25，0 / 25 |
| 44.1 kHz | 48.18% / 37.50% | 138.70 / 119.10 ms | 372.26 / 383.17 ms | 361.43 / 361.43 ms | 0 / 25，0 / 25 |

48 kHz 声样的 CPU/GPU 输出相关 0.9997819，RMS 差相对于 CPU 输出 RMS 为 2.169%；44.1 kHz 为 0.9996673 和 2.721%。这同时包含声样重采样、选段、运行库和提供者差异。每组离线信号偏移均为 50 ms；不能将其等同真实实时延迟。`quality-*.json` 含目标/干扰相关系数及试听音频哈希。

## 文件与可追溯性

- `quality-*.json` 为离线相关分析；包含本机运行时标识和时间的原始逐次测量文件不随仓库分发。
- 测试使用的混音与 CPU/GPU 输出均为合成语音；音频文件不随仓库分发，可参考下述工具在本地生成。对齐后的试听音频不能用来测实时延迟。
- 使用 `native/src/bin/realtime_compare.rs` 执行 Rust 全链，`native/benchmarks/prepare_gpu_fixture.py` 生成长合成混音，`compare_ipc_pipeline.py` 对齐、分析并生成试听。文件必须明确传入；不要用未授权私人音频。
- `enroll.onnx` SHA-256：`5d06048c156bb8cd422d29848e6d38fb26a7b39f6f7fd92eccf2a6ed1ce003c6`；`chunk_int8.onnx`：`df61a9d0062b0adbcf1182302b1eb8b815f15ac47ea4923e8e164c1aff64121d`；`gtcrn_simple.onnx`：`e77603ac0c23dac3227dd2d7135b3a585cbee2679048aecfa886657d3ae1b534`。
- CPU sherpa DLL SHA-256 `71728fb12b3a15a9ab7c4961fceafc6dd802f7fc038f461f5ea59e07d7104bec`，CPU ORT 1.28.2 DLL `422d776ab0e3218260f7f628fcb84606aaa5c21116f720c8619a6da5e0b2e0f9`；DirectML ORT 1.24.4 DLL `e7eedec6a6f26dc39dc948276a75ef6d2bee3fff944d874ceed0bbd3b97bff40`。
- 3 秒 16 kHz 声样 SHA-256 `9f84eedfcd514500c50add75c3da4d5136a5da72c071678eef5264d0a10a1703`，长合成混音 `4b8ed06c854a314874fc1f7398e809ddfdbbb5fd2fd7390bd5cabc81bcb75335`。试听源和完整测试 WAV 均为合成，不包含个人声样。
