# Windows x64 release packaging

The script assembles the current Rust program, four checked models, the sherpa-onnx CPU runtime, the DirectML worker and runtime, documentation, and third-party notices. It creates a portable ZIP, a per-user Inno Setup installer, and SHA-256 checksums under `native/target/release-assets/v2.1.0/`. It never includes a recorded voice profile. The outputs are unsigned.

Install Rust and the Windows GNU toolchain, a compatible MinGW-w64 `g++.exe`, and Inno Setup. Populate Cargo's cache with the dependencies in `Cargo.lock`, then run `native/setup.ps1` to download and verify the CPU runtime and models. The target machine needs the Microsoft Visual C++ 2015–2022 x64 Redistributable and a separately installed virtual audio cable; neither is silently installed by this package.

From the repository root, run:

```powershell
& ./native/packaging/build-release.ps1 -Cxx 'C:\path\to\g++.exe' -Iscc 'C:\path\to\ISCC.exe'
```

`setup-gpu.ps1` downloads a pinned Microsoft DirectML ONNX Runtime package and checks its SHA-256. To reuse a previously downloaded copy, append `-DirectMLPackage 'C:\path\to\microsoft.ml.onnxruntime.directml.1.24.4.nupkg'`. Cargo compilation and dependency metadata collection run offline after the cache is populated. On GNU toolchains, set `CARGO_TARGET_DIR` to an ASCII-only path if the linker cannot handle the project path.

Each invocation uses a fresh staging directory under `native/target`. Existing release output files are never overwritten: move them to an archive before rerunning. The model and CPU runtime hashes are checked before packaging. The Rust dependency inventory is generated for the building host target from the lockfile and includes license and notice files from each downloaded crate, including embedded font notices. The `licenses` directory contains additional upstream notices for components whose crate archive omits root license files and the runtime libraries.

The installer copies the same files as the portable package and creates per-user shortcuts. It does not auto-install system dependencies, alter audio device settings, or delete voice samples on uninstall. Closing the app to the tray leaves it running; use its Exit button or tray menu to stop it fully.
