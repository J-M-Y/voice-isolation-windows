# 处理块长基准

本目录测量当前 `src/processor.rs` 模型链的离线负载，不访问麦克风或个人声样，也不包含实时设备、界面和排队成本。每次启动独立处理器，预热 2.4 秒，再处理相同的 12 秒、48 kHz 单声道输入；每个模式和块长默认运行三轮。`results.jsonl` 记录进程 CPU 时间、墙钟时间和按输入音频时长折算的单逻辑核等效负载，浮点 WAV 可用于波形比较。

在项目根 PowerShell 中，先用本机 Python（需 NumPy、SciPy）生成合成语音：

```powershell
powershell -ExecutionPolicy Bypass -File native/generate_cpu_benchmark.ps1 -Python 'C:\path\to\python.exe'
```

安装 Rust 和项目模型、运行库后，运行：

```powershell
powershell -ExecutionPolicy Bypass -File native/benchmarks/profile_blocks.ps1 -Cargo 'C:\path\to\cargo.exe' -OutputDir "$env:TEMP\voice-block-example" -Trials 3
& 'C:\path\to\python.exe' native/benchmarks/compare_blocks.py "$env:TEMP\voice-block-example"
```

脚本默认读取 `%TEMP%\voice-isolation-cpu-benchmark\reference.wav` 和 `input.wav`，可通过 `-Reference`、`-InputWav`、`-Models` 指定其他显式测试文件；`-OutputDir` 应指向新的目录。默认使用当前 `CARGO_TARGET_DIR`，未设置时使用系统临时目录中的全英文路径。若 Cargo 为专用 rustup 安装，调用前需设置对应的 `RUSTUP_HOME`、`CARGO_HOME` 和编译器 PATH。

`compare_blocks.py` 以各档首轮输出的 80 ms WAV 为基准，报告 CPU 中位数、逐样本余弦、最大差、差值 RMS，以及 160/240 ms 块边界前后各 1 ms 的差值 RMS。它不测真实设备延迟、说话人筛选质量、长期稳定性，也不能排除不同模型分块形状在其他音频中的影响；应保持机器负载稳定并记录任何并发运行的程序。
