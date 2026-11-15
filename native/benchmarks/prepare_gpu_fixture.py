"""Create non-private, 30.24 s synthetic test files from supplied SAPI fixture."""
import argparse
from pathlib import Path
import numpy as np
from scipy.io import wavfile
from gpu_fixture import loop_with_crossfade, mono16

RATE = 48000
COUNT = round(32.64 * RATE)  # 2.4s warmup + 30.24s measurement


def read48(path):
    rate, x = wavfile.read(path)
    if rate != RATE or x.ndim != 1 or x.dtype != np.int16: raise ValueError(f'Expected mono 48 kHz PCM16: {path}')
    return x.astype(np.float32) / 32768


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--fixture', type=Path, required=True, help='Directory from generate_two_speaker_test.ps1')
    p.add_argument('--output-dir', type=Path, required=True)
    a = p.parse_args()
    a.output_dir.mkdir(parents=True, exist_ok=True)
    for stem in ('target','interference','mixture'):
        x = loop_with_crossfade(read48(a.fixture / f'{stem}.wav'), COUNT, fade_samples=4800)
        wavfile.write(a.output_dir / f'{stem}-long.wav', RATE, np.clip(np.rint(x * 32768),-32768,32767).astype(np.int16))
    ref = mono16(a.fixture / 'reference.wav')[:3*16000]
    if len(ref) != 3*16000: raise ValueError('Reference too short')
    wavfile.write(a.output_dir / 'reference-3s-16k.wav',16000,np.clip(np.rint(ref*32768),-32768,32767).astype(np.int16))
    print(f'Fixture created: {a.output_dir}', flush=True)

if __name__ == '__main__': main()
