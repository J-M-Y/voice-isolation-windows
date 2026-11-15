# Rust 原生版构建与测试

此目录是 Windows x64 原生实现。保留声样录制、VoxLock 指定说话人提取、GTCRN/DPDFNet2 降噪、音频设备选择、试听和托盘；现有 Python 版仍可使用，两者不能同时运行。模型和音频在本机处理。程序不安装虚拟声卡；需自行安装并选择 VB-CABLE 等虚拟声卡，在语音软件中选择其录音端。

原生版使用 sherpa-onnx 适配的 VoxLock 1.1.1 模型；旧 Python 版使用的 VoxLock 1.3 模型文件不能直接互换。两者的音质、说话人筛选效果与 CPU 占用需要实际对照。

## 构建与运行

1. 安装 Rust 和 Windows C/C++ 构建工具。推荐 x86_64-pc-windows-msvc 工具链和 Visual Studio C++ 桌面构建工具；GNU 工具链也需要完整的 MinGW binutils。
2. 在仓库根目录的 PowerShell 中运行 `powershell -ExecutionPolicy Bypass -File native/setup.ps1`。脚本从上游下载固定版本的 sherpa-onnx 2.0.1 运行库及模型，验证 SHA-256 后放入 `models` 和 `native/target/{debug,release}/runtime`；可重新运行以续传。下载需要网络，不会读取或修改个人声样。
3. 运行 `cargo build --release --locked --manifest-path native/Cargo.toml --bins`，然后启动 `native/target/release/voice-isolation-native.exe`。建议从仓库目录启动。模型推理不需要 Python。
4. 选择实体麦克风，点击“录入声音（12 秒）”；输出选择虚拟声卡的播放端（例如 CABLE Input），开始处理。在游戏或语音软件中选择对应的录音端（例如 CABLE Output）。先停止处理，再录制试听并通过实体耳机播放。

处理模式可以在停止时切换：**低延迟**、**省 CPU**和**更省 CPU**分别使用 80、160、240 毫秒音频块。较大音频块减少模型调用次数，但增加采集等待。三个模式还分别增加约 20、40、80 毫秒的播放预缓冲，用来吸收短时处理和设备调度波动；相对于原先没有预缓冲的版本，这部分会增加输出延迟。程序会记住所选处理模式。实际总延迟还包括设备、处理和排队时间。试听也使用当前所选模式；处理时如缓冲丢块或输出缺样，界面会显示计数和累计缺样时长（含恢复缓冲），而不会静默隐藏。

勾选显卡加速，可自动选卡或手动选择显示的设备；建议先选 240 毫秒“更省 CPU”档。声纹提取通过 Windows DirectML 在独立工作进程中运行，支持符合 DirectML 要求的 AMD、NVIDIA 和 Intel 显卡，无需安装 CUDA；自动选卡优先专用显存较大的硬件设备。手动选卡会核对设备标识，设备发生变化时需刷新列表。降噪及部分模型算子仍可能在 CPU 上运行；启动时验证首块和后续块确有 GPU 计算，失败则停止，不会悄悄切换设备或透传原声。首次启动需等待模型初始化。当前仅在 AMD Radeon RX 6800 上完成实机测试，NVIDIA 和其他型号还需实机验证。真实声卡、长时间运行和游戏 GPU 竞争下仍需试听及观察缓冲计数。

如需从源码准备 GPU 运行库，可使用 Windows x64 的 MinGW-w64 `g++.exe` 运行 `powershell -ExecutionPolicy Bypass -File native/setup-gpu.ps1 -Cxx <g++.exe路径>`，再启动上述步骤构建的 `native/target/release/voice-isolation-native.exe`。脚本从微软 NuGet 下载并校验固定版 DirectML ONNX Runtime，在 `native/target/release/runtime-dml` 构建独立工作进程，不覆盖 CPU 运行库。程序和 GPU worker 必须使用相同协议版本；运行旧显卡版时不要覆盖它的 worker，可用脚本的 `-Destination` 参数安装到另一个目录。DirectML 系统组件由 Windows 提供，运行环境还需要 [Visual C++ 2015–2022 x64 可再发行组件](https://learn.microsoft.com/zh-cn/cpp/windows/latest-supported-vc-redist)。

关闭或最小化窗口会进入托盘，右键托盘图标可退出。声样默认存于 `%LOCALAPPDATA%\VoiceIsolation\我的声样.wav`，与旧版共享；也可选择其他 WAV。两种降噪模式均先进行目标说话人提取，然后降噪。效果、延迟和 CPU 消耗取决于硬件与模型，不能保证完全滤掉其他声音；程序没有声学回声消除。

官方 sherpa-onnx 2.0.1 发布库通过 WAV 文件载入声样。程序会短暂创建一份临时 WAV，并在载入后删除；如果进程被强制终止，临时文件可能残留在系统临时目录中，文件名以 `voice-isolation-` 开头。程序只对校验过的官方 DLL 启用此兼容方式，并检查声样加载失败时不会直接透传原始音频。

## 无设备测试和性能测量

`cargo test --locked --manifest-path native/Cargo.toml` 运行单元测试。构建后的 `benchmark.exe <声样.wav> <48k输入.wav> <模型目录> <输出前缀>` 会分别运行两种模式，保存处理后的 WAV 并打印实时系数（处理耗时 ÷ 音频时长，越低越快）。请在同一台电脑、相同音频和模式下对照 Python 版；这项指标不是系统整体 CPU 占用率。运行库要位于 benchmark.exe 同级的 `runtime` 目录。

在一台 i7-12700KF（20 逻辑处理器）上，以 Windows 合成语音、80 ms 音频块、单线程模型推理、2 秒预热及每次 30 秒音频连续测量三次，按进程 CPU 时间除以音频时长计算，结果的中位数如下。数值是维持实时音频所需的单逻辑核等效负载，不包含音频设备、图形界面及其他系统活动：

| 模式 | 仓库 Python 版 | Rust 版 | Rust 相对变化 |
| --- | ---: | ---: | ---: |
| 普通（GTCRN） | 44.01% | 68.75% | 增加约 56.2% |
| 加强（DPDFNet2） | 56.67% | 80.05% | 增加约 41.3% |

上表中的 80 ms Rust 处理链没有比 Python 版降低 CPU。两版 VoxLock 模型版本不同，上表不能用来单独衡量语言开销；合成语音也不能证明真实环境中的音质和声纹筛选效果相同。测试工具为 `generate_cpu_benchmark.ps1`、`benchmark_cpu.py` 和 `cpu_benchmark.exe`，Python 对照需要原版 Python 依赖及原版模型。

随后对同一个 Rust 模型比较了三种处理块长，使用相同 12 秒合成语音，各测三次取中位数：

| 降噪 | 低延迟 80 ms | 省 CPU 160 ms | 更省 CPU 240 ms |
| --- | ---: | ---: | ---: |
| 普通 | 70.31% | 55.99% | 50.65% |
| 加强 | 82.68% | 64.19% | 58.98% |

百分比仍为单逻辑核等效的模型核心负载，包含采集等待的完整程序占用需在实际设备上测量；测试时旧应用仍在运行，可能影响调度和频率。较大块长的离线输出与 80 ms 档接近，但真实设备的延迟、音质和掉帧仍需试听。完整条件、结果及复现步骤见 [分块基准](benchmarks/README.md)。

两说话人合成混音测试可由 `generate_two_speaker_test.ps1` 与 `two_speaker_test.py` 重现；它需要 Windows SAPI 的 Zira 和 Huihui 声音以及 Python 的 NumPy、SciPy。该测试只覆盖合成语音样本，不代表真实多人、噪声或音乐环境。合成试听 WAV 未纳入 Git 仓库，需按照报告中的步骤在本地生成。

RX 6800 的正式 Rust 链离线对照（同一 V1 模型、普通降噪、240 ms 块、30.24 秒双人合成混音，各一轮）中，主进程与 worker 合计 CPU 时间由 15.72 秒降至 14.11 秒，约减少 10.2%；处理每块的 p95 从 161.00 ms 变为 177.31 ms，两组均 0/126 块晚于下一块采集期限。无丢包模拟的采集起点到输出包播放起点 p50 分别约 423 ms 和 447 ms；它不包含模型信号偏移、声卡、系统音频或网络，不能当作真实端到端延迟。两路输出在该合成样本上高度接近，但 GPU 声样重采样与 CPU 库不同，真实效果仍需验证。完整条件和音质数据见 [DirectML 测试报告](benchmarks/results/directml-rust-20261116/README.md)。

若 GNU 工具链在含中文的目录下报链接路径错误，请改用 MSVC，或将工具链与 Cargo 构建输出放到全英文路径，再将两个 exe 复制到 `native/target/release`。不要移动或覆盖声样。

运行库与模型的版权及来源见仓库根 README。发布时应同时提供各依赖许可证；`setup.ps1` 下载的是上游官方文件。
