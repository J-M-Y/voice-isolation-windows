"""Paced synthetic GPU-worker pipeline: 48k FIR -> V1 IPC -> GTCRN/DPDF -> 48k FIR.

No actual microphone, device callback, Windows Audio or GUI is measured. Parent
is a Python/SciPy simulation of the Rust streaming worker, so report its
process CPU separately from the DirectML child and do not claim these numbers
are production-process CPU. The background GPU performance counter sampler is
separate and its CPU, GPU driver services and other system processes are not
included. The source WAVs must be explicitly supplied synthetic test files.
"""
import argparse
import ctypes
from ctypes import wintypes
from collections import deque
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import struct
import subprocess
import threading
import time

import numpy as np
from scipy.io import wavfile
from scipy.signal import firwin, lfilter
from gpu_fixture import reference16, loop_with_crossfade

RATE = 48000
MODEL_RATE = 16000
WARM_SECONDS = 2.4
MEASURE_SECONDS = 30.24


def input48(path):
    rate, x = wavfile.read(path)
    if rate != RATE or x.dtype != np.int16 or x.ndim != 1:
        raise ValueError('Input must be mono 48000 Hz PCM16 WAV')
    return np.ascontiguousarray(x.astype(np.float32) / 32768)


class FileTime(ctypes.Structure):
    _fields_ = [('low', wintypes.DWORD), ('high', wintypes.DWORD)]


def child_cpu_seconds(process):
    creation, exit_time, kernel, user = (FileTime() for _ in range(4))
    k32 = ctypes.WinDLL('kernel32', use_last_error=True)
    k32.GetProcessTimes.argtypes = [wintypes.HANDLE, ctypes.POINTER(FileTime), ctypes.POINTER(FileTime), ctypes.POINTER(FileTime), ctypes.POINTER(FileTime)]
    k32.GetProcessTimes.restype = wintypes.BOOL
    if not k32.GetProcessTimes(wintypes.HANDLE(int(process._handle)), ctypes.byref(creation), ctypes.byref(exit_time), ctypes.byref(kernel), ctypes.byref(user)):
        raise ctypes.WinError(ctypes.get_last_error())
    ticks = lambda t: (t.high << 32) | t.low
    return (ticks(kernel) + ticks(user)) / 10_000_000


class Gtcrn(ctypes.Structure): _fields_ = [('model', ctypes.c_char_p)]
class Dpdf(ctypes.Structure): _fields_ = [('model', ctypes.c_char_p), ('attenuation', ctypes.c_float)]
class Vox(ctypes.Structure): _fields_ = [('enroll', ctypes.c_char_p), ('chunk', ctypes.c_char_p), ('voice_sample', ctypes.c_char_p)]
class Model(ctypes.Structure): _fields_ = [('gtcrn', Gtcrn), ('num_threads', ctypes.c_int32), ('debug', ctypes.c_int32), ('provider', ctypes.c_char_p), ('dpdf', Dpdf), ('vox', Vox)]
class Config(ctypes.Structure): _fields_ = [('model', Model)]
FloatPtr = ctypes.POINTER(ctypes.c_float)
class DenoisedAudio(ctypes.Structure): _fields_ = [('samples', FloatPtr), ('n', ctypes.c_int32), ('sample_rate', ctypes.c_int32)]


class Denoiser:
    def __init__(self, dll_path, model, strong):
        self.lib = ctypes.CDLL(str(dll_path.resolve()), winmode=0x00000100 | 0x00001000)
        self.lib.SherpaOnnxCreateOnlineSpeechDenoiser.argtypes = [ctypes.POINTER(Config)]
        self.lib.SherpaOnnxCreateOnlineSpeechDenoiser.restype = ctypes.c_void_p
        self.lib.SherpaOnnxOnlineSpeechDenoiserGetSampleRate.argtypes = [ctypes.c_void_p]
        self.lib.SherpaOnnxOnlineSpeechDenoiserGetSampleRate.restype = ctypes.c_int32
        self.lib.SherpaOnnxOnlineSpeechDenoiserGetFrameShiftInSamples.argtypes = [ctypes.c_void_p]
        self.lib.SherpaOnnxOnlineSpeechDenoiserGetFrameShiftInSamples.restype = ctypes.c_int32
        self.lib.SherpaOnnxOnlineSpeechDenoiserRun.argtypes = [ctypes.c_void_p, FloatPtr, ctypes.c_int32, ctypes.c_int32]
        self.lib.SherpaOnnxOnlineSpeechDenoiserRun.restype = ctypes.POINTER(DenoisedAudio)
        self.lib.SherpaOnnxDestroyDenoisedAudio.argtypes = [ctypes.POINTER(DenoisedAudio)]
        self.lib.SherpaOnnxDestroyOnlineSpeechDenoiser.argtypes = [ctypes.c_void_p]
        cfg = Config()
        cfg.model.num_threads = 1
        cfg.model.provider = b'cpu'
        if strong: cfg.model.dpdf.model = str(model.resolve()).encode('utf-8')
        else: cfg.model.gtcrn.model = str(model.resolve()).encode('utf-8')
        self.handle = self.lib.SherpaOnnxCreateOnlineSpeechDenoiser(ctypes.byref(cfg))
        if not self.handle: raise RuntimeError('Denoiser initialization failed')
        if self.lib.SherpaOnnxOnlineSpeechDenoiserGetSampleRate(self.handle) != MODEL_RATE:
            raise RuntimeError('Denoiser sample rate mismatch')
        self.delay = self.lib.SherpaOnnxOnlineSpeechDenoiserGetFrameShiftInSamples(self.handle)
        if self.delay < 0: raise RuntimeError('Denoiser delay invalid')

    def push(self, samples):
        p = self.lib.SherpaOnnxOnlineSpeechDenoiserRun(self.handle, samples.ctypes.data_as(FloatPtr), len(samples), MODEL_RATE)
        if not p: return np.empty(0, np.float32)
        try:
            result = p.contents
            if result.n < 0 or result.sample_rate != MODEL_RATE or (result.n and not result.samples): raise RuntimeError('Invalid denoiser output')
            return np.ctypeslib.as_array(result.samples, shape=(result.n,)).copy() if result.n else np.empty(0, np.float32)
        finally:
            self.lib.SherpaOnnxDestroyDenoisedAudio(p)

    def close(self):
        if self.handle:
            self.lib.SherpaOnnxDestroyOnlineSpeechDenoiser(self.handle)
            self.handle = None


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ('worker', 'gpu-runtime', 'sherpa-dll', 'enroll', 'chunk', 'denoiser', 'reference', 'input', 'output', 'result', 'gpu-samples', 'stderr'):
        p.add_argument('--' + name, type=Path, required=True)
    p.add_argument('--provider', choices=('cpu','directml'), required=True)
    p.add_argument('--mode', choices=('normal','strong'), required=True)
    p.add_argument('--block-ms', type=int, choices=(80,160,240), required=True)
    p.add_argument('--adapter', type=int, default=0)
    p.add_argument('--seconds', type=float, default=MEASURE_SECONDS)
    p.add_argument('--sample-gpu', action='store_true')
    p.add_argument('--protocol', choices=('legacy','checked'), default='checked')
    args = p.parse_args()
    block = args.block_ms * RATE // 1000
    model_block = block // 3
    warm_count, measure_count = round(WARM_SECONDS * RATE), round(args.seconds * RATE)
    if not 0 < args.seconds <= 300 or warm_count % block or measure_count % block:
        raise ValueError('Measurement duration must be positive and divisible by block size')
    raw = input48(args.input)
    audio = loop_with_crossfade(raw, warm_count + measure_count, fade_samples=4800)
    ref = reference16(args.reference)
    if len(ref) < 3 * MODEL_RATE: raise ValueError('Synthetic reference is shorter than 3 seconds')
    # Processor's official-API fallback writes PCM16 with Rust's truncating
    # float-to-int conversion; reproduce its 3-second enrollment exactly.
    ref = np.ascontiguousarray(np.clip(np.trunc(ref[:3 * MODEL_RATE] * 32767), -32768, 32767).astype(np.int16).astype(np.float32) / 32768)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.result.parent.mkdir(parents=True, exist_ok=True)
    args.stderr.parent.mkdir(parents=True, exist_ok=True)
    gpu_log = args.gpu_samples
    if gpu_log.exists(): raise ValueError(f'GPU sample output already exists: {gpu_log}')
    stderr = args.stderr.open('wb')
    cmd = [str(args.worker.resolve()), str(args.gpu_runtime.resolve()), str(args.enroll.resolve()), str(args.chunk.resolve()),
           'dml' if args.provider == 'directml' else 'cpu', str(args.adapter), str(model_block)]
    worker = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr, bufsize=0,
                              creationflags=subprocess.CREATE_NO_WINDOW)
    sampler = None
    timed_out = threading.Event()
    deadline = [time.monotonic() + 90]
    finish = threading.Event()
    def watchdog():
        while not finish.wait(0.05):
            limit = deadline[0]
            if limit is not None and time.monotonic() > limit:
                timed_out.set()
                try: worker.kill()
                except OSError: pass
                return
    threading.Thread(target=watchdog, daemon=True).start()
    def read_exact(n):
        result = bytearray()
        while len(result) < n:
            data = worker.stdout.read(n - len(result))
            if not data: raise RuntimeError('GPU worker timed out' if timed_out.is_set() else f'GPU worker ended unexpectedly ({worker.poll()})')
            result.extend(data)
        return bytes(result)
    def write_all(data):
        view = memoryview(data)
        while view:
            n = worker.stdin.write(view)
            if not n: raise RuntimeError('GPU worker pipe closed')
            view = view[n:]
    def read_status():
        n = struct.unpack('<I', read_exact(4))[0]
        if n == 0xffffffff:
            size = struct.unpack('<I', read_exact(4))[0]
            if size > 4096: raise RuntimeError('Invalid worker error length')
            raise RuntimeError(read_exact(size).decode('utf-8', errors='replace'))
        return n
    try:
        if args.sample_gpu:
            sampler_script = Path(__file__).with_name('sample_gpu_engine.ps1')
            sampler = subprocess.Popen(['powershell.exe','-NoProfile','-ExecutionPolicy','Bypass','-File',str(sampler_script.resolve()),
                                        '-TargetPid',str(worker.pid),'-OutputPath',str(gpu_log.resolve()),'-MaxSeconds','180'],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=subprocess.CREATE_NO_WINDOW)
        initialization_start = time.perf_counter()
        if args.protocol == 'checked':
            magic = struct.unpack('<I', read_exact(4))[0]
            if magic != 0x31555047: raise RuntimeError('Unexpected checked worker protocol')
        write_all(ref.astype('<f4', copy=False).tobytes())
        if read_status() != 0: raise RuntimeError('Invalid initialization handshake')
        device_name = 'unverified by legacy protocol'
        selected_adapter = args.adapter
        if args.protocol == 'checked':
            selected_adapter, name_len = struct.unpack('<II', read_exact(8))
            if name_len > 4096: raise RuntimeError('Invalid worker device name length')
            device_name = read_exact(name_len).decode('utf-8', errors='replace')
        worker_initialization_seconds = time.perf_counter() - initialization_start
        deadline[0] = None
        denoiser = Denoiser(args.sherpa_dll, args.denoiser, args.mode == 'strong')
        kernel = firwin(97, 0.30).astype(np.float32)
        denominator = np.array([1.0], dtype=np.float32)
        down_state = np.zeros(96, dtype=np.float32)
        up_state = np.zeros(96, dtype=np.float32)
        extract_queue = deque([0.0] * 512)
        denoise_queue = deque([0.0] * denoiser.delay)
        starvation = [0, 0]
        def take(queue, ready, count, which):
            queue.extend(ready)
            missing = max(0, count - len(queue))
            starvation[which] += missing
            result = np.zeros(count, dtype=np.float32)
            for i in range(missing, count): result[i] = queue.popleft()
            return result
        def process(samples):
            nonlocal down_state, up_state
            begin = time.perf_counter()
            filtered, down_state = lfilter(kernel, denominator, samples, zi=down_state)
            down = np.ascontiguousarray(filtered[::3], dtype=np.float32)
            before_ipc = time.perf_counter()
            deadline[0] = time.monotonic() + max(3.0, args.block_ms / 1000 * 10)
            write_all(struct.pack('<I', len(down)))
            write_all(down.astype('<f4', copy=False).tobytes())
            n = read_status()
            if n > model_block + 1024: raise RuntimeError('Invalid model output length')
            ready = np.frombuffer(read_exact(n * 4), dtype='<f4').copy()
            deadline[0] = None
            ipc_ms = (time.perf_counter() - before_ipc) * 1000
            extracted = take(extract_queue, ready, len(down), 0)
            before_denoise = time.perf_counter()
            clean = take(denoise_queue, denoiser.push(extracted), len(down), 1)
            denoise_ms = (time.perf_counter() - before_denoise) * 1000
            zeros = np.zeros(block, dtype=np.float32)
            zeros[::3] = clean * 3
            output, up_state = lfilter(kernel, denominator, zeros, zi=up_state)
            if len(output) != block or not np.isfinite(output).all(): raise RuntimeError('Invalid pipeline output')
            return output, (time.perf_counter()-begin)*1000, ipc_ms, denoise_ms
        try:
            warm_start = time.perf_counter()
            for offset in range(0, warm_count, block): process(audio[offset:offset+block])
            warm_wall_seconds = time.perf_counter() - warm_start
            starvation[:] = [0,0]
            output = np.empty(measure_count, np.float32)
            block_seconds = args.block_ms / 1000
            process_ms, ipc_ms, denoise_ms, late, play_delays = [], [], [], [], []
            playback_end = 0.0
            child_before, parent_before = child_cpu_seconds(worker), time.process_time()
            start_utc, started = datetime.now(timezone.utc).isoformat(), time.perf_counter()
            for i, offset in enumerate(range(warm_count, warm_count + measure_count, block)):
                deadline_capture = started + (i+1) * block_seconds
                pause = deadline_capture - time.perf_counter()
                if pause > 0: time.sleep(pause)
                result, elapsed, wait_ipc, denoise = process(audio[offset:offset+block])
                output[i*block:(i+1)*block] = result
                done = time.perf_counter()
                process_ms.append(elapsed); ipc_ms.append(wait_ipc); denoise_ms.append(denoise)
                late.append(max(0, (done - (deadline_capture + block_seconds)) * 1000))
                play_start = max(done - started, playback_end)
                play_delays.append((play_start - i*block_seconds) * 1000)
                playback_end = play_start + block_seconds
            wall, parent_cpu, child_cpu = time.perf_counter()-started, time.process_time()-parent_before, child_cpu_seconds(worker)-child_before
            end_utc = datetime.now(timezone.utc).isoformat()
            pct = lambda values, p: float(np.percentile(values,p))
            result = {'provider':args.provider, 'mode':args.mode, 'block_ms':args.block_ms,
                      'pid_parent':os.getpid(), 'pid_child':worker.pid, 'adapter':selected_adapter, 'device_name':device_name,
                      'input_seconds':args.seconds, 'warmup_seconds':WARM_SECONDS, 'wall_seconds':wall,
                      'worker_initialization_seconds':worker_initialization_seconds, 'warm_wall_seconds':warm_wall_seconds,
                      'measurement_start_utc':start_utc, 'measurement_end_utc':end_utc,
                      'parent_cpu_seconds':parent_cpu, 'child_cpu_seconds':child_cpu, 'combined_cpu_seconds':parent_cpu+child_cpu,
                      'combined_single_core_percent':100*(parent_cpu+child_cpu)/args.seconds,
                      'process_ms_p50':pct(process_ms,50), 'process_ms_p95':pct(process_ms,95), 'process_ms_p99':pct(process_ms,99),
                      'ipc_roundtrip_ms_p50':pct(ipc_ms,50), 'ipc_roundtrip_ms_p95':pct(ipc_ms,95), 'ipc_roundtrip_ms_p99':pct(ipc_ms,99),
                      'denoise_ms_p50':pct(denoise_ms,50), 'denoise_ms_p95':pct(denoise_ms,95),
                      'deadline_misses':sum(x>0 for x in late), 'block_count':len(late), 'deadline_late_ms_max':max(late),
                      'simulated_capture_to_packet_play_ms_p50':pct(play_delays,50), 'simulated_capture_to_packet_play_ms_p95':pct(play_delays,95),
                      'simulated_capture_to_packet_play_ms_max':max(play_delays), 'extract_starved_samples':starvation[0], 'denoise_starved_samples':starvation[1],
                      'output_rms':float(np.sqrt(np.mean(output.astype(np.float64)**2))), 'process_ms_by_block':process_ms, 'ipc_ms_by_block':ipc_ms,
                      'deadline_late_ms_by_block':late, 'fixture_repeat':'100ms linear crossfade',
                      'scope':'Python/SciPy parent + isolated model worker + stock CPU denoiser; no actual audio devices/UI; no GPU driver service CPU; no-drop simulated playback'}
            wavfile.write(args.output, RATE, output)
            args.result.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
            print(json.dumps(result, ensure_ascii=False), flush=True)
        finally:
            denoiser.close()
    finally:
        deadline[0] = None
        try:
            if worker.poll() is None:
                write_all(struct.pack('<I', 0))
                worker.stdin.close()
                worker.wait(timeout=5)
        except Exception:
            if worker.poll() is None: worker.kill()
        finish.set()
        if sampler:
            try: sampler.wait(timeout=5)
            except subprocess.TimeoutExpired: sampler.terminate()
        stderr.close()

if __name__ == '__main__': main()
