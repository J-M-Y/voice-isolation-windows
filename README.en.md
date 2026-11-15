# Voice Isolation for Windows

[English](README.md) · [简体中文](README.zh-CN.md) · [Downloads / 下载](https://github.com/J-M-Y/voice-isolation-windows/releases/latest) · [Changelog / 版本记录](CHANGELOG.md)

Extract a selected speaker's voice and reduce noise from a microphone locally on Windows. Enroll your voice, then send the processed audio to a virtual audio cable for use in games, voice chats, or meetings. The current release is a native Rust application and does not require Python at runtime. Audio processing and the enrolled voice sample stay on your computer.

## Features

- Record or import a WAV voice sample, extract the selected speaker, and choose normal or stronger background-music filtering.
- Choose input, output, and preview devices; record a five-second sample to compare the original with the processed audio.
- Select **Low latency**, **Lower CPU**, or **Lowest CPU**. Larger processing blocks reduce model calls but add waiting time.
- Run on the CPU or use Windows DirectML with compatible AMD, NVIDIA, and Intel GPUs. Select a GPU automatically or manually; CUDA is not required. Some model operators and noise reduction still run on the CPU.
- Keep processing in the system tray and see dropped-block and output-underrun counts, including the total time spent filling missing output with silence.

Quality, CPU use, and latency depend on the hardware and environment. The application cannot guarantee complete removal of other speakers, music, or noise, and does not provide dedicated acoustic echo cancellation. The DirectML path is vendor-neutral and has been tested on an AMD Radeon RX 6800; NVIDIA and other GPU models have not yet been tested on physical hardware.

## Download and installation

Download the Windows x64 installer or portable ZIP from [GitHub Releases](https://github.com/J-M-Y/voice-isolation-windows/releases/latest). For the portable version, extract the entire archive and run `VoiceIsolation.exe`; keep the bundled model and runtime directories alongside the executable. Neither package requires Python. If your system does not already have it, install Microsoft's [Visual C++ 2015–2022 x64 Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist).

The application does not install a virtual audio device. Install [VB-CABLE](https://vb-audio.com/Cable/) or another compatible virtual audio cable separately. Installing a third-party driver may require administrator permission and a restart. VB-CABLE is a separate product by VB-Audio; its [license](https://vb-audio.com/Services/licensing.htm) governs its use and redistribution.

GPU acceleration requires Windows 10 version 1903 or later, a DirectX 12 / DirectML-capable GPU, and a suitable driver. Support for the model and its operators can vary by device. If initialization or verification of actual GPU execution fails, the application reports the error; disable GPU acceleration to use the CPU. See the [DirectML requirements](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html).

## Getting started

1. Select your physical microphone and click **录入声音（12 秒）** (Record voice, 12 seconds). Speak normally and continuously in a quiet place, or import an existing WAV sample.
2. Select the playback end of your virtual cable as the output device, for example **CABLE Input**, and click **开始处理** (Start processing).
3. In your game or voice app, select the cable's recording end, for example **CABLE Output**, as the microphone. Keep your normal headphones as the playback device.
4. Stop processing before using the five-second preview; select physical headphones or speakers to compare the original and processed audio.

Closing or minimizing the window keeps the application in the system tray. Double-click the tray icon to restore the window; use **退出软件** (Exit) in the tray menu or main window to stop processing and exit. Expand the taskbar's hidden-icons area if necessary. Exit an older running version before starting a new one. The application interface is currently in Chinese.

The three modes use 80, 160, and 240 ms audio blocks, plus approximately 20, 40, and 80 ms of playback pre-buffering respectively. Actual end-to-end latency also includes model, device, system, and network delays. Continuously increasing drop or underrun counts can indicate audible gaps; reduce system load or try another processing mode.

The default sample is saved at `%LOCALAPPDATA%\VoiceIsolation\我的声样.wav`; another WAV may be selected. Audio is not uploaded during normal processing. The CPU runtime briefly creates a temporary WAV while loading the sample and deletes it afterward; a forced termination may leave that temporary file behind. Voice samples are not included in the source or release packages.

## Build from source

You need Windows x64, Rust, and Windows C/C++ build tools. From the repository root:

```powershell
powershell -ExecutionPolicy Bypass -File native/setup.ps1
cargo build --release --locked --manifest-path native/Cargo.toml --bins
```

`setup.ps1` downloads and verifies pinned model files and the CPU runtime. For GPU support, also run `native/setup-gpu.ps1 -Cxx <path-to-g++.exe>` with a Windows MinGW-w64 compiler to prepare the isolated DirectML runtime and worker. See the [Rust build notes](https://github.com/J-M-Y/voice-isolation-windows/blob/main/native/README.md) for setup, tests, model sources, and performance measurements.

The repository retains the older Python implementation. It uses a different VoxLock model version from the Rust application; the models are not interchangeable, and the two applications cannot run simultaneously.

## Third-party components

The application depends on [VoxLock](https://github.com/arbazsiddiqui/voxlock), [GTCRN](https://github.com/Xiaobin-Rong/gtcrn), [DPDFNet2](https://huggingface.co/Ceva-IP/DPDFNet), [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx), ONNX Runtime, and other components. Their respective authors own their copyrights and licenses; see the third-party notices in the release package. Virtual audio drivers are not part of this project.
