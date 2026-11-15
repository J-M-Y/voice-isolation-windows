"""Paced offline V1 stream benchmark; no capture devices or private audio.

Run only with a known, verified native/experimental_gpu/vox_v1.dll. A run
warms the same stream for 2.4 s, then plays 30.24 s of the supplied WAV on a
real-time capture clock. CPU time belongs to this process only. Model timings
exclude device callbacks, resampling, denoising, actual playback and GPU-driver
service processes. See --help for arguments.
"""
import argparse
import ctypes
from collections import deque
from datetime import datetime, timezone
import json
import os
import tempfile
import time
from pathlib import Path

import numpy as np
from scipy.io import wavfile
from gpu_fixture import mono16, loop_with_crossfade

RATE = 16000
WARM_SECONDS = 2.4
MEASURE_SECONDS = 30.24


def process_seconds():
    # Python's Windows process_time uses kernel+user CPU time from GetProcessTimes.
    return time.process_time()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ('backend', 'runtime', 'enroll', 'chunk', 'reference', 'input', 'output', 'result'):
        p.add_argument('--' + name, type=Path, required=True)
    p.add_argument('--provider', choices=('cpu', 'directml', 'sherpa'), required=True,
                   help='sherpa uses the official current-program CPU extractor as a reference')
    p.add_argument('--adapter', type=int, default=0)
    p.add_argument('--block-ms', type=int, choices=(80, 160, 240), required=True)
    p.add_argument('--fixed', action='store_true', help='Use experimental fixed-shape model sessions')
    p.add_argument('--seconds', type=float, default=MEASURE_SECONDS,
                   help='Formal tests use 30.24 seconds; shorter durations are only for smoke checks')
    args = p.parse_args()
    ref = mono16(args.reference)
    source = mono16(args.input)
    if len(ref) < 3 * RATE or not len(source):
        raise ValueError('Reference must be >=3 s and input must be nonempty')
    # Both providers receive identical 3 seconds; no microphone access.
    # Exactly the same PCM16 enrollment reaches both the stock sherpa WAV API
    # and the experimental float-PCM C ABI.
    ref_pcm = np.clip(np.rint(ref[:3 * RATE] * 32767), -32768, 32767).astype(np.int16)
    ref = np.ascontiguousarray(ref_pcm.astype(np.float32) / 32768)
    if not 0 < args.seconds <= 300:
        raise ValueError('seconds must be in (0, 300]')
    needed = round((WARM_SECONDS + args.seconds) * RATE)
    audio = loop_with_crossfade(source, needed)
    block = args.block_ms * RATE // 1000
    warm_count, measured_count = round(WARM_SECONDS * RATE), round(args.seconds * RATE)
    if warm_count % block or measured_count % block:
        raise ValueError('Internal test durations must be divisible by block size')
    ptr = ctypes.POINTER(ctypes.c_float)
    init_start = time.perf_counter()
    dll = ctypes.CDLL(str(args.backend.resolve()), winmode=0x00000100 | 0x00001000)
    if args.provider == 'sherpa':
        class Gtcrn(ctypes.Structure): _fields_ = [('model', ctypes.c_char_p)]
        class Dpdf(ctypes.Structure): _fields_ = [('model', ctypes.c_char_p), ('attenuation', ctypes.c_float)]
        class Vox(ctypes.Structure): _fields_ = [('enroll', ctypes.c_char_p), ('chunk', ctypes.c_char_p), ('voice_sample', ctypes.c_char_p)]
        class Model(ctypes.Structure): _fields_ = [('gtcrn', Gtcrn), ('num_threads', ctypes.c_int32), ('debug', ctypes.c_int32), ('provider', ctypes.c_char_p), ('dpdf', Dpdf), ('vox', Vox)]
        class Config(ctypes.Structure): _fields_ = [('model', Model)]
        class DenoisedAudio(ctypes.Structure): _fields_ = [('samples', ptr), ('n', ctypes.c_int32), ('sample_rate', ctypes.c_int32)]
        dll.SherpaOnnxCreateOnlineSpeechDenoiser.argtypes = [ctypes.POINTER(Config)]
        dll.SherpaOnnxCreateOnlineSpeechDenoiser.restype = ctypes.c_void_p
        dll.SherpaOnnxOnlineSpeechDenoiserGetSampleRate.argtypes = [ctypes.c_void_p]
        dll.SherpaOnnxOnlineSpeechDenoiserGetSampleRate.restype = ctypes.c_int32
        dll.SherpaOnnxOnlineSpeechDenoiserGetFrameShiftInSamples.argtypes = [ctypes.c_void_p]
        dll.SherpaOnnxOnlineSpeechDenoiserGetFrameShiftInSamples.restype = ctypes.c_int32
        dll.SherpaOnnxOnlineSpeechDenoiserRun.argtypes = [ctypes.c_void_p, ptr, ctypes.c_int32, ctypes.c_int32]
        dll.SherpaOnnxOnlineSpeechDenoiserRun.restype = ctypes.POINTER(DenoisedAudio)
        dll.SherpaOnnxDestroyDenoisedAudio.argtypes = [ctypes.POINTER(DenoisedAudio)]
        dll.SherpaOnnxOnlineSpeechDenoiserReset.argtypes = [ctypes.c_void_p]
        dll.SherpaOnnxDestroyOnlineSpeechDenoiser.argtypes = [ctypes.c_void_p]
        # The fixture is synthetic; the temporary file is removed as soon as
        # the API has copied the enrollment. Never use a personal recording.
        with tempfile.TemporaryDirectory(prefix='voice-gpu-test-') as td:
            ref_wav = Path(td) / 'synthetic-reference.wav'
            wavfile.write(ref_wav, RATE, ref_pcm)
            cfg = Config()
            cfg.model.num_threads = 1
            cfg.model.provider = b'cpu'
            cfg.model.vox.enroll = str(args.enroll.resolve()).encode('utf-8')
            cfg.model.vox.chunk = str(args.chunk.resolve()).encode('utf-8')
            cfg.model.vox.voice_sample = str(ref_wav).encode('utf-8')
            handle = dll.SherpaOnnxCreateOnlineSpeechDenoiser(ctypes.byref(cfg))
        if not handle:
            raise RuntimeError('Stock sherpa extractor failed to initialize')
        if dll.SherpaOnnxOnlineSpeechDenoiserGetSampleRate(handle) != RATE or dll.SherpaOnnxOnlineSpeechDenoiserGetFrameShiftInSamples(handle) != 128:
            dll.SherpaOnnxDestroyOnlineSpeechDenoiser(handle)
            raise RuntimeError('Unexpected official extractor model configuration')
        probe = np.array([0.3125], dtype=np.float32)
        returned = dll.SherpaOnnxOnlineSpeechDenoiserRun(handle, probe.ctypes.data_as(ptr), 1, RATE)
        if returned:
            dll.SherpaOnnxDestroyDenoisedAudio(returned)
            dll.SherpaOnnxDestroyOnlineSpeechDenoiser(handle)
            raise RuntimeError('Stock sherpa did not preload the synthetic enrollment')
        dll.SherpaOnnxOnlineSpeechDenoiserReset(handle)
        def push(samples):
            result = dll.SherpaOnnxOnlineSpeechDenoiserRun(handle, samples.ctypes.data_as(ptr), len(samples), RATE)
            if not result: return np.empty(0, dtype=np.float32)
            try:
                item = result.contents
                if item.n < 0 or item.sample_rate != RATE or (item.n and not item.samples):
                    raise RuntimeError('Invalid sherpa result')
                return np.ctypeslib.as_array(item.samples, shape=(item.n,)).copy() if item.n else np.empty(0, dtype=np.float32)
            finally: dll.SherpaOnnxDestroyDenoisedAudio(result)
        def destroy(): dll.SherpaOnnxDestroyOnlineSpeechDenoiser(handle)
    else:
        create = dll.vox_v1_create_fixed if args.fixed else dll.vox_v1_create
        create.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_wchar_p, ptr, ctypes.c_size_t,
                           ctypes.c_int, ctypes.c_int] + ([ctypes.c_size_t] if args.fixed else []) + [ctypes.c_char_p, ctypes.c_size_t]
        create.restype = ctypes.c_void_p
        dll.vox_v1_push.argtypes = [ctypes.c_void_p, ptr, ctypes.c_size_t, ptr, ctypes.c_size_t,
                                   ctypes.POINTER(ctypes.c_size_t), ctypes.c_char_p, ctypes.c_size_t]
        dll.vox_v1_push.restype = ctypes.c_int
        dll.vox_v1_destroy.argtypes = [ctypes.c_void_p]
        dll.vox_v1_destroy.restype = None
        error = ctypes.create_string_buffer(4096)
        create_args = [str(args.runtime.resolve()), str(args.enroll.resolve()), str(args.chunk.resolve()),
                       ref.ctypes.data_as(ptr), len(ref), int(args.provider == 'directml'), args.adapter]
        if args.fixed: create_args.append(block)
        handle = create(*create_args, error, len(error))
        if not handle:
            raise RuntimeError(error.value.decode('utf-8', errors='replace'))
        output_buffer = np.empty(block + 4096, dtype=np.float32)
        def push(samples):
            written = ctypes.c_size_t()
            error.value = b''
            status = dll.vox_v1_push(handle, samples.ctypes.data_as(ptr), len(samples),
                                      output_buffer.ctypes.data_as(ptr), len(output_buffer),
                                      ctypes.byref(written), error, len(error))
            if status: raise RuntimeError(error.value.decode('utf-8', errors='replace'))
            if written.value > len(output_buffer): raise RuntimeError('Backend wrote invalid sample count')
            return output_buffer[:written.value]
        def destroy(): dll.vox_v1_destroy(handle)
    initialization_seconds = time.perf_counter() - init_start
    fifo = deque([0.0] * 512)  # matches the current Rust extractor cushion at 16 kHz
    starved_samples = 0

    def one(samples):
        nonlocal starved_samples
        fifo.extend(push(samples))
        missing = max(0, block - len(fifo))
        starved_samples += missing
        out = np.zeros(block, dtype=np.float32)
        for i in range(missing, block):
            out[i] = fifo.popleft()
        if not np.isfinite(out).all():
            raise RuntimeError('Non-finite output')
        return out

    try:
        warm_start = time.perf_counter()
        warm_timings = []
        for i in range(0, warm_count, block):
            before = time.perf_counter()
            one(audio[i:i + block])
            warm_timings.append((time.perf_counter() - before) * 1000)
        warm_wall_seconds = time.perf_counter() - warm_start
        starved_samples = 0
        block_s = args.block_ms / 1000
        timings, deadline_lateness, simulated_start_delays = [], [], []
        rendered = np.empty(measured_count, dtype=np.float32)
        playback_end = 0.0
        start_utc = datetime.now(timezone.utc).isoformat()
        start_cpu = process_seconds()
        start = time.perf_counter()
        for n, offset in enumerate(range(warm_count, warm_count + measured_count, block)):
            # A captured block is available only at the end of its input interval.
            capture_deadline = start + (n + 1) * block_s
            delay = capture_deadline - time.perf_counter()
            if delay > 0:
                time.sleep(delay)
            before = time.perf_counter()
            rendered[n * block:(n + 1) * block] = one(audio[offset:offset + block])
            after = time.perf_counter()
            done = after - start
            timings.append((after - before) * 1000)
            deadline_lateness.append(max(0.0, after - (capture_deadline + block_s)) * 1000)
            # A no-drop simulation of packet playback. Actual device latency,
            # model waveform delay, resampling and denoising are not included.
            playback_start = max(done, playback_end)
            simulated_start_delays.append((playback_start - n * block_s) * 1000)
            playback_end = playback_start + block_s
        wall = time.perf_counter() - start
        cpu = process_seconds() - start_cpu
        def pct(values, percent): return float(np.percentile(np.asarray(values), percent))
        result = dict(pid=os.getpid(), provider=args.provider, adapter=args.adapter, block_ms=args.block_ms, fixed=args.fixed,
                      initialization_seconds=initialization_seconds, warm_wall_seconds=warm_wall_seconds,
                      warm_process_ms=warm_timings,
                      fixture_repeat='100ms linear crossfade',
                      warmup_seconds=WARM_SECONDS, input_seconds=args.seconds, wall_seconds=wall,
                      measurement_start_utc=start_utc, measurement_end_utc=datetime.now(timezone.utc).isoformat(),
                      cpu_seconds=cpu, single_core_percent=cpu / args.seconds * 100,
                      process_ms_p50=pct(timings,50), process_ms_p95=pct(timings,95), process_ms_p99=pct(timings,99),
                      process_ms_max=max(timings), deadline_misses=sum(v > 0 for v in deadline_lateness),
                      process_ms_by_block=timings, deadline_late_ms_by_block=deadline_lateness,
                      deadline_late_ms_max=max(deadline_lateness),
                      simulated_capture_to_packet_play_ms_p50=pct(simulated_start_delays,50),
                      simulated_capture_to_packet_play_ms_p95=pct(simulated_start_delays,95),
                      simulated_capture_to_packet_play_ms_max=max(simulated_start_delays),
                      rendered_starved_samples=starved_samples,
                      output_rms=float(np.sqrt(np.mean(rendered.astype(np.float64) ** 2))),
                      scope='VoxLock model only; excludes denoiser, audio I/O and GPU driver service CPU')
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.result.parent.mkdir(parents=True, exist_ok=True)
        wavfile.write(args.output, RATE, rendered)
        args.result.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
        print(json.dumps(result, ensure_ascii=False), flush=True)
    finally:
        destroy()


if __name__ == '__main__':
    main()
