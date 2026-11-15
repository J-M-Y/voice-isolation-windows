"""Shared fixture helpers for paired, synthetic V1 DirectML experiments."""
import math
import numpy as np
from scipy.io import wavfile
from scipy.signal import resample_poly

RATE = 16000


def mono16(path):
    rate, data = wavfile.read(path)
    if data.ndim != 1 or rate != 48000 or data.dtype != np.int16:
        raise ValueError(f'{path} must be mono 16-bit 48000 Hz WAV')
    return np.ascontiguousarray(resample_poly(data.astype(np.float32) / 32768, 1, 3), dtype=np.float32)


def reference16(path):
    rate, data = wavfile.read(path)
    if data.ndim != 1 or data.dtype != np.int16 or rate not in (16000, 44100, 48000):
        raise ValueError(f'{path} must be mono 16-bit PCM at 16kHz, 44.1kHz or 48kHz')
    if rate == 16000:
        return np.ascontiguousarray(data.astype(np.float32) / 32768)
    if rate == 44100:
        return np.ascontiguousarray(resample_poly(data.astype(np.float32) / 32768, 160, 441), dtype=np.float32)
    return mono16(path)


def loop_with_crossfade(source, required, fade_samples=1600):
    """Repeat a fixture with 100-ms linear crossfades, preserving level.

    Applying this linear operation to the two stems individually preserves
    their sum (apart from their original PCM quantization).
    """
    source = np.asarray(source, dtype=np.float32)
    if source.ndim != 1 or len(source) <= fade_samples or required <= 0:
        raise ValueError('Source too short or invalid requested length')
    result = source.copy()
    weights = np.linspace(0, 1, fade_samples, endpoint=False, dtype=np.float32)
    while len(result) < required:
        result[-fade_samples:] = result[-fade_samples:] * (1 - weights) + source[:fade_samples] * weights
        result = np.concatenate((result, source[fade_samples:]))
    return np.ascontiguousarray(result[:required], dtype=np.float32)
