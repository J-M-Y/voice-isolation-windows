# Voice Isolation for Windows

[English](README.md) · [简体中文](README.zh-CN.md) · [Downloads / 下载](https://github.com/J-M-Y/voice-isolation-windows/releases/latest) · [Changelog / 版本记录](CHANGELOG.md)

Keep game voice chat focused on your voice while music, short videos, or other background sounds play nearby. Enroll your voice and send the processed microphone audio to a virtual audio cable, so teammates hear less of those sounds. Processing runs locally on Windows; the native Rust application does not require Python at runtime.

## Features

- Record or import a WAV voice sample, extract the selected speaker, and choose normal or stronger background-music filtering.
- Switch the application interface between English and Simplified Chinese; your language choice is saved.
- Choose input, output, and preview devices; record a five-second sample to compare the original with the processed audio.
- Select **Low latency**, **Lower CPU**, or **Lowest CPU**. Larger processing blocks reduce model calls but add waiting time.
- Run on the CPU or use Windows DirectML with compatible AMD, NVIDIA, and Intel GPUs. Select a GPU automatically or manually; CUDA is not required. Some model operators and noise reduction still run on the CPU.
- Keep processing in the system tray and see dropped-block and output-underrun counts, including the total time spent filling missing output with silence.

Only microphone input is processed; the application does not remove music or video from your own speakers. Quality, CPU use, and latency depend on the hardware and environment. Complete removal of background sounds is not guaranteed, and the application does not provide dedicated acoustic echo cancellation. The DirectML path is vendor-neutral and has been tested on an AMD Radeon RX 6800; NVIDIA and other GPU models have not yet been tested on physical hardware.

## Download and installation

Download the Windows x64 installer or portable ZIP from [GitHub Releases](https://github.com/J-M-Y/voice-isolation-windows/releases/latest). For the portable version, extract the entire archive and run `VoiceIsolation.exe`; keep the bundled model and runtime directories alongside the executable. Neither package requires Python. If your system does not already have it, install Microsoft's [Visual C++ 2015–2022 x64 Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist).

The application does not install a virtual audio device. Install [VB-CABLE](https://vb-audio.com/Cable/) or another compatible virtual audio cable separately. Installing a third-party driver may require administrator permission and a restart. VB-CABLE is a separate product by VB-Audio; its [license](https://vb-audio.com/Services/licensing.htm) governs its use and redistribution.

GPU acceleration requires Windows 10 version 1903 or later, a DirectX 12 / DirectML-capable GPU, and a suitable driver. Support for the model and its operators can vary by device. If initialization or verification of actual GPU execution fails, the application reports the error; disable GPU acceleration to use the CPU. See the [DirectML requirements](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html).

## Getting started

1. Use the language selector at the top of the window to choose **English** or **简体中文**. The initial choice follows the Windows display language; changes take effect immediately and are saved.
2. Select your physical microphone and click **Record voice (12 s)**. Speak normally and continuously in a quiet place, or import an existing WAV sample.
3. Select the playback end of your virtual cable as the output device, for example **CABLE Input**, and click **Start processing**.
4. In your game or voice app, select the cable's recording end, for example **CABLE Output**, as the microphone. Keep your normal headphones as the playback device.
5. Stop processing before using the five-second preview; select physical headphones or speakers to compare the original and processed audio.

Closing or minimizing the window keeps the application in the system tray. Double-click the tray icon to restore the window; use **Exit** in the tray menu or main window to stop processing and exit. Expand the taskbar's hidden-icons area if necessary. Exit an older running version before starting a new one.

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
