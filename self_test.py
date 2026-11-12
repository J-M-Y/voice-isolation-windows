"""Non-interactive installed-build check using an explicitly supplied WAV.

This test deliberately never uses microphone devices, network, or a user's
enrollment profile. The caller supplies the audio file and the JSON result path.
"""
from pathlib import Path
import json
import math

import numpy as np
from scipy import signal

import voice_model


def run_self_test(reference, result_file):
    result = {"ok": False, "modes": {}}
    try:
        source = voice_model.voxlock.load_audio(str(reference))
        if source.ndim != 1 or len(source) < 3 * 16000 or not np.isfinite(source).all():
            raise ValueError("self-test WAV must contain at least 3 seconds of finite mono audio")
        # The model expects 48-kHz input. A short copy of the explicitly
        # supplied sample exercises exactly the same processors as live mode.
        samples = signal.resample_poly(source, 3, 1).astype(np.float32)
        block = 3840
        total = 30 * block
        if len(samples) < total:
            samples = np.pad(samples, (0, total - len(samples)))
        samples = samples[:total]
        for strong in (False, True):
            processor = voice_model.create_processor(str(reference), 48000, block, strong_music=strong)
            try:
                output = np.concatenate([processor.process(samples[i:i + block])
                                         for i in range(0, total, block)])
            finally:
                processor.close()
            if output.shape != (total,) or not np.isfinite(output).all():
                raise RuntimeError("model returned invalid audio")
            rms = float(np.sqrt(np.mean(output.astype(np.float64) ** 2)))
            if not math.isfinite(rms) or rms <= 1e-8:
                raise RuntimeError("model produced only silence")
            result["modes"]["strong" if strong else "normal"] = {
                "samples": len(output), "rms": rms,
            }
        result["ok"] = True
    except Exception as exc:
        result["error"] = f"{type(exc).__name__}: {exc}"
    Path(result_file).write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    return 0 if result["ok"] else 1
