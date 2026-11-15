"""Measure the existing Python processor's steady-state CPU time offline.

This reads only explicitly supplied test WAVs. It excludes model initialization,
uses the application's 80 ms blocks, warms up for 2 s, and measures 30 s of
input per trial. The source directory selects the original Python version and
its adjacent model directory, not the sherpa-specific Rust model directory.
"""
import argparse
import importlib.util
import json
import time
from pathlib import Path

import numpy as np
from scipy.io import wavfile

BLOCK = 3840
RATE = 48000


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--source', required=True, type=Path)
    parser.add_argument('--reference', required=True, type=Path)
    parser.add_argument('--input', required=True, type=Path)
    parser.add_argument('--mode', required=True, choices=('gtcrn', 'dpdfnet2'))
    parser.add_argument('--trials', type=int, default=3)
    args = parser.parse_args()
    rate, audio = wavfile.read(args.input)
    if rate != RATE or audio.ndim != 1 or audio.dtype != np.int16:
        raise ValueError('Input must be mono 16-bit WAV at 48000 Hz')
    x = audio.astype(np.float32) / 32768.0
    warm = 25 * BLOCK
    count = 375 * BLOCK
    if len(x) < warm + count:
        raise ValueError('Input must be at least 32 seconds')
    spec = importlib.util.spec_from_file_location('bench_voice_model', args.source / 'voice_model.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    processor = module.create_processor(str(args.reference), RATE, BLOCK, strong_music=args.mode == 'dpdfnet2')
    try:
        for i in range(0, warm, BLOCK):
            processor.process(x[i:i+BLOCK])
        samples = x[warm:warm+count]
        out = np.empty(count, dtype=np.float32)
        for trial in range(args.trials):
            cpu0 = time.process_time()
            wall0 = time.perf_counter()
            for i in range(0, count, BLOCK):
                out[i:i+BLOCK] = processor.process(samples[i:i+BLOCK])
            wall = time.perf_counter() - wall0
            cpu = time.process_time() - cpu0
            if not np.isfinite(out).all():
                raise RuntimeError('Non-finite samples')
            rms = float(np.sqrt(np.mean(out.astype(np.float64)**2)))
            print(json.dumps(dict(mode=args.mode, trial=trial+1, audio_seconds=count/RATE,
                                  wall_seconds=wall, cpu_seconds=cpu, cpu_one_core_percent=100*cpu/(count/RATE),
                                  realtime_factor=wall/(count/RATE), output_rms=rms)), flush=True)
    finally:
        processor.close()

if __name__ == '__main__':
    main()
