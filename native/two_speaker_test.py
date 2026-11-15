"""Prepare and analyze a two-SAPI-voice fixture, never using a microphone.

After `generate_two_speaker_test.ps1`, run the Rust benchmark with
reference.wav and mixture.wav, output prefix `output`. Then call this file
with `analyze <directory>`. Correlation is a narrow signal test, not a
comprehensive speech-separation or recognition-quality metric.
"""
import json
import sys
from pathlib import Path

import numpy as np
from scipy.io import wavfile
from scipy.signal import correlate, correlation_lags, resample_poly

RATE = 48000


def read(path):
    rate, samples = wavfile.read(path)
    if samples.ndim != 1 or samples.dtype != np.int16 or rate != 22050:
        raise ValueError(f'{path} must be mono 16-bit SAPI WAV at 22050 Hz')
    return resample_poly(samples.astype(np.float32) / 32768.0, 320, 147)


def prepare(directory):
    ref = read(directory / 'ref-source.wav')[:6 * RATE]
    target = read(directory / 'target-source.wav')[:16 * RATE]
    other = read(directory / 'interference-source.wav')[:16 * RATE]
    if len(ref) < 6 * RATE or len(target) < 16 * RATE or len(other) < 16 * RATE:
        raise ValueError('SAPI speech is shorter than the fixed fixture durations')
    target = target * (0.12 / np.sqrt(np.mean(target.astype(np.float64) ** 2)))
    other = other * (0.12 / np.sqrt(np.mean(other.astype(np.float64) ** 2)))
    gain = min(1.0, 0.95 / np.max(np.abs(target + other)))
    target *= gain
    other *= gain
    for name, audio in [('reference.wav', ref), ('target.wav', target),
                        ('interference.wav', other), ('mixture.wav', target + other)]:
        pcm = np.clip(np.rint(audio * 32767), -32768, 32767).astype(np.int16)
        wavfile.write(directory / name, RATE, pcm)
    print(json.dumps({'seconds': 16, 'target_rms': float(np.sqrt(np.mean(target ** 2))),
                      'interference_rms': float(np.sqrt(np.mean(other ** 2)))}))


def audio(path):
    rate, samples = wavfile.read(path)
    if rate != RATE or samples.ndim != 1:
        raise ValueError(f'{path} must be mono 48000 Hz WAV')
    if samples.dtype == np.int16:
        return samples.astype(np.float64) / 32768.0
    if samples.dtype == np.float32:
        return samples.astype(np.float64)
    raise ValueError(f'Unsupported format in {path}')


def cosine(a, b):
    denominator = np.linalg.norm(a) * np.linalg.norm(b)
    if denominator == 0:
        raise ValueError('Cannot compute correlation on silence')
    return float(np.dot(a, b) / denominator)


def analyze(directory):
    target = audio(directory / 'target.wav')
    other = audio(directory / 'interference.wav')
    for mode in ('gtcrn', 'dpdfnet2'):
        output = audio(directory / f'output-{mode}.wav')
        if len(target) != len(other) or len(target) != len(output):
            raise ValueError('Fixture WAV lengths differ')
        corr = correlate(output, target, mode='full', method='fft')
        lags = correlation_lags(len(output), len(target))
        allowed = (lags >= 0) & (lags <= 3 * RATE)
        lag = int(lags[allowed][np.argmax(np.abs(corr[allowed]))])
        start, stop = 2 * RATE, len(output) - lag
        if stop <= start:
            raise ValueError('Insufficient aligned audio')
        x, y = target[start:stop], other[start:stop]
        z = output[start + lag:stop + lag]
        if not np.isfinite(z).all():
            raise ValueError('Non-finite model output')
        print(json.dumps({'mode': mode, 'lag_ms': lag / RATE * 1000,
                          'target_output_cosine': cosine(x, z),
                          'interference_output_cosine': cosine(y, z),
                          'target_mixture_cosine': cosine(x, x + y),
                          'interference_mixture_cosine': cosine(y, x + y),
                          'aligned_seconds': len(z) / RATE}))


if __name__ == '__main__':
    if len(sys.argv) != 3 or sys.argv[1] not in ('prepare', 'analyze'):
        raise SystemExit('usage: two_speaker_test.py prepare|analyze directory')
    {'prepare': prepare, 'analyze': analyze}[sys.argv[1]](Path(sys.argv[2]))
