# 合成双说话人试听：Rust CPU 与 DirectML

输入是 Windows SAPI 合成的 Zira 目标声音和 Huihui 干扰声音，与个人录音无关。混音时两人整体 RMS 相等，处理模式为 240 毫秒音频块与普通 GTCRN 降噪。两份处理结果均来自 Rust 离线节拍处理链；GPU 使用独立 worker 和 RX 6800 的 DirectML。以下音频文件不随 Git 仓库分发，可用上级测试工具在本地生成。

- `01-synthetic-mix.wav`：未处理的混音。
- `02-cpu-reference.wav`：CPU 版处理结果。
- `03-gpu-experimental.wav`：当时 DirectML 处理结果（保留历史测试文件名）。

为了方便听声音，输出已按目标语音离线对齐，三段使用同一倍增益并截取约 12 秒，**这些文件不能衡量真实实时延迟**。此样本仅覆盖一组安静合成双人语音；真实口音、音乐、键盘噪声、设备延迟和漏字仍需另测。`quality.json` 是音频相关分析。
