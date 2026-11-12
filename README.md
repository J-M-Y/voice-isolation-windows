# 指定人声提取 · Windows 实验版

一款 Windows 本地人声提取与降噪小工具。录入一段自己的声音后，可实时处理麦克风输入并输出到虚拟声卡，供游戏或语音软件使用。最小化或关闭窗口后，程序留在系统托盘，处理可以继续；从托盘退出时停止处理。

> 这是实验软件。处理效果取决于麦克风、环境、说话音量和电脑性能，无法保证完全去除音乐或其他人的声音，也可能出现漏字、失真和延迟。当前版本不包含专门的声学回声消除功能。

## 下载安装

前往本仓库的 **Releases** 页面下载 Windows 安装包或便携版。安装包不需要 Python；便携版解压后运行 `VoiceIsolation.exe`。程序不要求将虚拟声卡设置为系统默认设备。

如果电脑还没有虚拟声卡，按照包内说明安装 VB-CABLE 并按提示重启。安装驱动需管理员权限。VB-CABLE 是 VB-Audio 的独立软件，属于 donationware；支持及捐赠见 [VB-Audio 官方网站](https://vb-audio.com/Cable/)，机构使用及再分发请阅读 [官方授权说明](https://vb-audio.com/Services/licensing.htm)。

## 使用方法

1. 选择自己的实体麦克风，在安静环境点击 **录入声音（12 秒）**，正常连续说话。
2. 保持自动选中的虚拟声卡输出端 `CABLE Input`，点击 **开始处理**。
3. 在游戏或语音软件中，将麦克风输入选择为 **CABLE Output**，耳机输出保持原来的设备。
4. 可点击 **试听前后效果** 检查效果。关闭窗口或最小化后程序会进入托盘；点击托盘图标可恢复窗口，右键选择“退出软件”会停止处理并退出。看不到图标时展开任务栏右下角的 `∧`。

程序及模型在本机运行。声样保存在本机 `%LOCALAPPDATA%\VoiceIsolation\我的声样.wav`，不会随安装包或源码发布；如需删除，可退出程序后自行删除该文件。软件只处理运行它的电脑发出的麦克风声音。朋友那边的环境噪声，需要朋友在自己的电脑上使用并录入自己的声音。

## 从源码运行

本仓库不包含模型权重或虚拟声卡驱动。安装 Python 3.12，运行 `repair_environment.bat` 安装依赖，再运行 `安装模型.bat` 下载模型，最后运行 `start.bat`。下载依赖和模型需要联网，正常音频处理在本地执行。其他构建步骤见 `installer/BUILD.md`。

## 第三方组件

- [VoxLock](https://github.com/arbazsiddiqui/voxlock) / [VoxLock 模型](https://huggingface.co/arbazsiddiqui/voxlock)：Apache-2.0。
- [GTCRN](https://github.com/Xiaobin-Rong/gtcrn)：MIT；应用使用 sherpa-onnx 提供的模型。
- [DPDFNet2](https://huggingface.co/Ceva-IP/DPDFNet)：Apache-2.0；应用使用 sherpa-onnx 提供的模型。
- [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)：Apache-2.0。
- VB-CABLE 版权归 VB-Audio 所有，不属于本项目源码。

第三方许可和声明见 `third_party_notices`；打包版本包含更完整的依赖许可证和来源信息。
