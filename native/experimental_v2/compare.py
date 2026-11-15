"""Device-free VoxLock V2 comparison. Supply synthetic/public WAV paths explicitly.

Run python and native modes in separate processes (they load different ORT DLLs),
then run compare. Results concern the extractor alone, not the complete app.
"""
import argparse
import ctypes
import json
import time
from pathlib import Path

import numpy as np
from scipy.io import wavfile
from scipy.signal import resample_poly

RATE = 16000
BLOCK = 1280


def read_audio(path):
    rate, data = wavfile.read(path)
    if rate != 48000 or data.ndim != 1 or data.dtype != np.int16:
        raise ValueError('expected mono PCM16 WAV at 48000 Hz')
    return np.ascontiguousarray(resample_poly(data.astype(np.float32) / 32768.0, 1, 3), dtype=np.float32)


def reference_window(data):
    if len(data) < 3 * RATE:
        raise ValueError('reference must be at least 3 seconds')
    data = data[:30 * RATE]
    count = len(data) // 160
    energies = (data[:count * 160].reshape(count, 160) ** 2).mean(axis=1)
    start = int(np.convolve(energies, np.ones(300), 'valid').argmax()) * 160
    return np.ascontiguousarray(data[start:start + 3 * RATE], dtype=np.float32)


class Native:
    def __init__(self, dll, runtime, models, ref):
        lib = ctypes.CDLL(str(dll))
        self.lib = lib
        lib.vox_v2_create.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_wchar_p,
                                     ctypes.POINTER(ctypes.c_float), ctypes.c_size_t,
                                     ctypes.c_char_p, ctypes.c_size_t]
        lib.vox_v2_create.restype = ctypes.c_void_p
        lib.vox_v2_push.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_float), ctypes.c_size_t,
                                   ctypes.POINTER(ctypes.c_float), ctypes.c_size_t,
                                   ctypes.POINTER(ctypes.c_size_t), ctypes.c_char_p, ctypes.c_size_t]
        lib.vox_v2_push.restype = ctypes.c_int
        lib.vox_v2_destroy.argtypes = [ctypes.c_void_p]
        error = ctypes.create_string_buffer(2048)
        self.handle = lib.vox_v2_create(str(runtime), str(models / 'enroll.onnx'), str(models / 'chunk_int8.onnx'),
                                        ref.ctypes.data_as(ctypes.POINTER(ctypes.c_float)), len(ref), error, len(error))
        if not self.handle:
            raise RuntimeError(error.value.decode('utf-8', errors='replace'))

    def push(self, samples):
        samples = np.ascontiguousarray(samples, dtype=np.float32)
        output = np.empty(len(samples) + 512, dtype=np.float32)
        written = ctypes.c_size_t()
        error = ctypes.create_string_buffer(2048)
        ret = self.lib.vox_v2_push(self.handle, samples.ctypes.data_as(ctypes.POINTER(ctypes.c_float)), len(samples),
                                   output.ctypes.data_as(ctypes.POINTER(ctypes.c_float)), len(output),
                                   ctypes.byref(written), error, len(error))
        if ret:
            raise RuntimeError(error.value.decode('utf-8', errors='replace'))
        return output[:written.value]

    def close(self):
        if self.handle:
            self.lib.vox_v2_destroy(self.handle)
            self.handle = None


def run(args):
    ref = reference_window(read_audio(args.reference))
    data = read_audio(args.input)
    measured = args.seconds * RATE
    if measured <= 0 or measured % BLOCK or len(data) < 2 * RATE + measured:
        raise ValueError('input must contain 2 seconds of warm-up plus the requested duration, in 80 ms blocks')
    if args.mode == 'native':
        if not args.dll or not args.runtime:
            raise ValueError('native mode requires --dll and --runtime')
        stream = Native(args.dll.resolve(), args.runtime.resolve(), args.models.resolve(), ref)
    else:
        import voxlock
        vl = voxlock.VoxLock(model_dir=args.models, threads=1)
        stream = vl.stream(ref)
    try:
        output = []
        for i in range(0, 2 * RATE, BLOCK):
            output.append(stream.push(data[i:i + BLOCK]))
        cpu0, wall0 = time.process_time(), time.perf_counter()
        for i in range(2 * RATE, 2 * RATE + measured, BLOCK):
            output.append(stream.push(data[i:i + BLOCK]))
        wall, cpu = time.perf_counter() - wall0, time.process_time() - cpu0
        output = np.concatenate(output)
        if not np.isfinite(output).all():
            raise ValueError('non-finite output')
        np.save(args.output, output)
        print(json.dumps({'mode': args.mode, 'samples': len(output), 'cpu_seconds': cpu,
                          'wall_seconds': wall, 'single_core_equivalent_percent': cpu / args.seconds * 100,
                          'rms': float(np.sqrt(np.mean(output.astype(np.float64)**2)))}))
    finally:
        if args.mode == 'native':
            stream.close()


def compare(args):
    first, second = np.load(args.python_output), np.load(args.native_output)
    if len(first) != len(second):
        raise ValueError(f'output lengths differ: {len(first)} vs {len(second)}')
    error = (first - second).astype(np.float64)
    signal = first.astype(np.float64)
    print(json.dumps({'samples': len(first), 'max_abs_error': float(np.max(np.abs(error))),
                      'error_rms': float(np.sqrt(np.mean(error**2))),
                      'reference_rms': float(np.sqrt(np.mean(signal**2))),
                      'relative_error_rms': float(np.sqrt(np.sum(error**2) / np.sum(signal**2)))}))


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest='mode', required=True)
    for mode in ('python', 'native'):
        p = sub.add_parser(mode)
        p.add_argument('--reference', type=Path, required=True)
        p.add_argument('--input', type=Path, required=True)
        p.add_argument('--models', type=Path, required=True)
        p.add_argument('--output', type=Path, required=True)
        p.add_argument('--seconds', type=int, default=30)
        if mode == 'native':
            p.add_argument('--dll', type=Path, required=True)
            p.add_argument('--runtime', type=Path, required=True)
    p = sub.add_parser('compare')
    p.add_argument('--python-output', type=Path, required=True)
    p.add_argument('--native-output', type=Path, required=True)
    args = parser.parse_args()
    (compare if args.mode == 'compare' else run)(args)


if __name__ == '__main__':
    main()
