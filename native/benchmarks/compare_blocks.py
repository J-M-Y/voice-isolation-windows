"""Summarize JSONL timings and compare trial-1 output waveform boundaries."""
import json
import sys
from pathlib import Path
from statistics import median

import numpy as np
from scipy.io import wavfile

def main(root):
    lines = (root / 'results.jsonl').read_text(encoding='utf-8-sig').splitlines()
    rows = [json.loads(line) for line in lines if line.strip()]
    for mode in ('normal', 'strong'):
        rate, baseline = wavfile.read(root / f'{mode}_3840_trial1.wav')
        baseline = baseline.astype(np.float64)
        for block in (3840, 7680, 11520):
            runs = [r for r in rows if r['mode'] == mode and r['block_samples'] == block]
            if not runs: raise ValueError(f'No results for {mode}/{block}')
            r, wave = wavfile.read(root / f'{mode}_{block}_trial1.wav')
            if r != rate or wave.shape != baseline.shape: raise ValueError('WAV format mismatch')
            wave = wave.astype(np.float64)
            diff = baseline - wave
            denom = np.linalg.norm(baseline) * np.linalg.norm(wave)
            if not denom: raise ValueError('Silent output')
            mask = np.zeros(len(wave), dtype=bool)
            for boundary in range(block, len(wave), block):
                mask[max(0, boundary - 48):min(len(wave), boundary + 48)] = True
            rms = lambda x: float(np.sqrt(np.mean(x*x)))
            print(json.dumps({'mode': mode, 'block_samples': block, 'trials': len(runs),
                'cpu_percent_median': median(r['cpu_one_core_percent'] for r in runs),
                'cpu_percent_trials': [r['cpu_one_core_percent'] for r in runs],
                'wall_seconds_median': median(r['wall_seconds'] for r in runs),
                'cosine_vs_80ms': float(np.dot(baseline, wave) / denom),
                'max_abs_difference': float(np.max(np.abs(diff))),
                'difference_rms': rms(diff), 'boundary_difference_rms': rms(diff[mask]),
                'nonboundary_difference_rms': rms(diff[~mask])}))

if __name__ == '__main__':
    if len(sys.argv) != 2: raise SystemExit('usage: compare_blocks.py output_dir')
    main(Path(sys.argv[1]))
