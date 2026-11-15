# Checked DirectML worker protocol

The worker is launched without a shell using absolute paths:

```
vox_v1_worker_checked.exe <ort.dll> <enroll.onnx> <chunk.onnx> dml <adapter-id|-1-auto> <block-size-16khz> <expected-luid|->
```

`block-size-16khz` must be 1280, 2560, or 3840. All integers and float32 samples are little-endian. stdout is exclusively protocol data; diagnostics go to stderr, which must be drained or redirected. The parent must enforce timeouts and terminate/reap the worker on EOF, malformed data, or timeout.

1. Worker sends u32 magic `0x32555047` (bytes `GPU2`). The parent verifies this **before** sending any voice sample. A GPU1 worker is incompatible and must be rejected.
2. Parent writes exactly 48,000 finite float32 samples (3 seconds, mono 16 kHz). For explicit selection, the original DXGI adapter index and the exact 16-digit hexadecimal LUID obtained from the most recent enumeration must both match; if the device has changed, startup fails rather than selecting a different card. For automatic selection pass `-1` and `-` as the index and LUID; the hardware adapter with the greatest dedicated memory is preferred (ties preserve the lowest DXGI index), and failure does not fall back to another adapter or CPU. The worker excludes DXGI software adapters and exercises first and steady fixed-shape sessions with a generated voice and zero audio under ORT profiling. Both sessions must execute non-copy DirectML nodes. This verification takes several seconds and never profiles the user's voice.
3. Worker returns u32 status: `0` is followed by u32 DXGI adapter index, u32 UTF-8 device-name length (1–1024), and that many UTF-8 bytes. `0xffffffff` is followed by u32 error byte length (up to 4096) and UTF-8 error bytes, then the worker exits.
4. For each block parent sends u32 count and that many finite float32 samples. Count must equal the configured block size. Worker returns u32 output count followed by float32 samples, or an error packet as above. The current fixed-size model must return `block_size - 384` samples on the first call and `block_size` samples on later calls; any other count or non-finite audio is invalid. The caller must maintain the same ordered, bounded FIFO as the CPU model.
5. Parent sends u32 zero to request clean exit. If the worker fails, times out, or emits malformed data, the caller must stop processing and never pass microphone data through as fallback.

`gpu_worker.rs` supplies an implementation with a hidden child, a Windows kill-on-close job, a 3 s protocol handshake timeout, a 30 s model initialization timeout, a 3 s per-block timeout, and bounded cleanup. It reports child CPU time separately; add it to parent CPU when comparing full-chain cost. This is not a stable external API.

## Adapter enumeration

Run `vox_v1_worker_checked.exe --list-adapters` to enumerate without loading a model or providing audio. Successful stdout is ASCII TSV with a `GPU2-LIST\t1` header and zero to 128 entries. Each entry is `index\tLUID\tvendor_id\tdevice_id\tname_hex`, where index is the original decimal DXGI index; LUID is 16 hexadecimal digits (`HighPart` then `LowPart`); vendor/device are 8 hexadecimal digits; and `name_hex` encodes the UTF-8 device-name bytes, at most 1023 bytes. DXGI software adapters are omitted. LUID is for checking a current enumeration, not for identifying hardware persistently across Windows restarts. Enumeration is not proof of DirectML/model support: the selected adapter must still pass the initialization and node checks above.
