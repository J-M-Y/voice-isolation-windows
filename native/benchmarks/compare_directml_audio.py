"""Compare CPU/DML V1 output on explicitly supplied synthetic mixture.

Input/target/interference WAVs must be the original 48 kHz SAPI fixture;
outputs are the 16 kHz float WAVs from directml_stream_test.py. The test
repeats the fixture and skips 2.4 s warmup exactly as that program does.
The measured signal lag excludes capture buffering, compute time, resampling,
denoising, real playback and audio device latency.
"""
import argparse
import json
from pathlib import Path

import numpy as np
from scipy.io import wavfile
from scipy.signal import correlate, correlation_lags
from gpu_fixture import mono16, loop_with_crossfade

RATE = 16000
WARM = round(2.4 * RATE)
COUNT = round(30.24 * RATE)


def input_wav(path):
    x = mono16(path)
    return loop_with_crossfade(x, WARM + COUNT)[WARM:WARM + COUNT].astype(np.float64)


def output_wav(path):
    rate, x = wavfile.read(path)
    if rate != RATE or x.ndim != 1 or x.dtype != np.float32 or len(x) != COUNT or not np.isfinite(x).all():
        raise ValueError(f'Expected finite 16kHz mono float WAV with {COUNT} samples: {path}')
    return x.astype(np.float64)


def cosine(a, b):
    den = np.linalg.norm(a) * np.linalg.norm(b)
    if den <= 1e-15: raise ValueError('Insufficient signal for correlation')
    return float(np.dot(a, b) / den)


def evaluate(target, other, out):
    # A fixed search window prevents alignment from fitting unrelated late speech.
    corr = correlate(out, target, mode='full', method='fft')
    lags = correlation_lags(len(out), len(target))
    allowed = (lags >= 0) & (lags <= 3 * RATE)
    lag = int(lags[allowed][np.argmax(np.abs(corr[allowed]))])
    lo, hi = 2 * RATE, min(len(target), len(out) - lag)
    if hi <= lo: raise ValueError('Insufficient aligned audio')
    x, y, z = target[lo:hi], other[lo:hi], out[lo + lag:hi + lag]
    # Linear projections are illustrative for this single nonlinear mixture.
    gains = np.linalg.lstsq(np.stack((x, y), axis=1), z, rcond=None)[0]
    return {'lag_samples': lag, 'signal_lag_ms': 1000 * lag / RATE,
            'target_correlation': cosine(x, z), 'interference_correlation': cosine(y, z),
            'target_linear_gain': float(gains[0]), 'interference_linear_gain': float(gains[1]),
            'output_rms': float(np.sqrt(np.mean(z*z))), 'aligned_seconds': (hi - lo) / RATE}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ('target', 'interference', 'cpu', 'gpu', 'output-dir'):
        p.add_argument('--' + name, type=Path, required=True)
    args = p.parse_args()
    x, y = input_wav(args.target), input_wav(args.interference)
    cpu, gpu = output_wav(args.cpu), output_wav(args.gpu)
    a, b = evaluate(x, y, cpu), evaluate(x, y, gpu)
    lo = 2 * RATE
    hi = min(len(x), len(cpu) - a['lag_samples'], len(gpu) - b['lag_samples'])
    ac, bc = cpu[lo+a['lag_samples']:hi+a['lag_samples']], gpu[lo+b['lag_samples']:hi+b['lag_samples']]
    diff = ac - bc
    base = float(np.sqrt(np.mean(ac * ac)))
    report = {'cpu': a, 'gpu': b, 'cpu_gpu_correlation': cosine(ac, bc),
              'cpu_gpu_difference_rms': float(np.sqrt(np.mean(diff * diff))),
              'cpu_gpu_difference_relative_rms': float(np.sqrt(np.mean(diff * diff)) / base) if base else None,
              'scope': 'synthetic speech, extraction model only; signal lag excludes real-time capture, compute, denoiser and audio-device delays'}
    args.output_dir.mkdir(parents=True, exist_ok=True)
    (args.output_dir / 'quality.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
    # All three previews use a single common gain; separate normalization could
    # obscure a difference in retained interference or target amplitude.
    count = min(12 * RATE, hi - lo)
    clips = {'synthetic-mixture.wav': (x + y)[lo:lo + count],
             'synthetic-cpu.wav': cpu[lo + a['lag_samples']:lo + a['lag_samples'] + count],
             'synthetic-gpu.wav': gpu[lo + b['lag_samples']:lo + b['lag_samples'] + count]}
    peak = max(float(np.max(np.abs(clip))) for clip in clips.values())
    common = 0.90 / peak if peak else 1.0
    for name, clip in clips.items():
        wavfile.write(args.output_dir / name, RATE, np.clip(np.rint(clip * common * 32767), -32768, 32767).astype(np.int16))
    report['preview_common_gain'] = common
    print(json.dumps(report, ensure_ascii=False), flush=True)

if __name__ == '__main__':
    main()
