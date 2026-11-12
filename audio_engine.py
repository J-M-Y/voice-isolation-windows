"""Real-time audio transport for the user interface."""
import queue
import threading
import time
import wave
import sys
from contextlib import contextmanager

import numpy as np
import sounddevice as sd


def compatible_audio_settings(device):
    """Allow Windows shared audio to adapt a device's native format to 48 kHz.

    WASAPI otherwise rejects many devices configured at a different sample
    rate (e.g. 44.1 kHz). Other host APIs do not accept WASAPI settings.
    """
    if sys.platform != "win32":
        return None
    endpoint = sd.query_devices(device)
    host = sd.query_hostapis(endpoint["hostapi"])
    if host["name"] == "Windows WASAPI":
        return sd.WasapiSettings(auto_convert=True)
    return None


@contextmanager
def com_initialized():
    """Initialize Windows COM on this thread; only balance our successful call."""
    if sys.platform != "win32":
        yield
        return
    import ctypes
    ole32 = ctypes.windll.ole32
    result = ole32.CoInitializeEx(None, 0)  # COINIT_MULTITHREADED
    # RPC_E_CHANGED_MODE means COM already exists in this thread as STA; it
    # remains usable, and must not be balanced by a CoUninitialize from us.
    already_other_mode = result == -2147417850
    if result < 0 and not already_other_mode:
        raise OSError(f"初始化 Windows 音频失败（COM 错误 0x{result & 0xffffffff:08X}）")
    try:
        yield
    finally:
        if result >= 0:
            ole32.CoUninitialize()


def record_reference(device, sample_rate, seconds, path):
    """Record a mono, 16-bit PCM WAV reference. Blocks the calling thread."""
    audio = sd.rec(int(seconds * sample_rate), samplerate=sample_rate,
                   channels=1, dtype="float32", device=device, blocking=True,
                   extra_settings=compatible_audio_settings(device))
    if not np.isfinite(audio).all() or np.sqrt(np.mean(audio.astype(np.float64) ** 2)) < 0.001:
        raise ValueError("录音音量过低或无效，请检查所选麦克风后重新录制")
    pcm = (np.clip(audio[:, 0], -1, 1) * 32767).astype("<i2")
    with wave.open(str(path), "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(sample_rate)
        wav.writeframes(pcm.tobytes())


class AudioEngine:
    def __init__(self, input_device, output_device, processor, sample_rate=48000,
                 block_size=3840, on_error=None):
        self.input_device = input_device
        self.output_device = output_device
        self.processor = processor
        self.sample_rate = sample_rate
        self.block_size = block_size
        self.on_error = on_error or (lambda message: None)
        self.input_queue = queue.Queue(maxsize=4)
        self.output_queue = queue.Queue(maxsize=4)
        self.stop_event = threading.Event()
        self.worker = None
        self.input_stream = None
        self.output_stream = None
        self.input_drops = 0
        self.output_drops = 0
        self.output_starved = 0
        self.callback_problems = 0
        self.started_at = None

    @staticmethod
    def _put_latest(q, value):
        try:
            q.put_nowait(value)
            return False
        except queue.Full:
            try:
                q.get_nowait()
            except queue.Empty:
                pass
            try:
                q.put_nowait(value)
            except queue.Full:
                pass
            return True

    def _input_callback(self, data, frames, time_info, status):
        if status:
            self.callback_problems += 1
        if self._put_latest(self.input_queue, data[:, 0].copy()):
            self.input_drops += 1

    def _output_callback(self, data, frames, time_info, status):
        if status:
            self.callback_problems += 1
        try:
            block = self.output_queue.get_nowait()
            data[:, 0] = block
        except queue.Empty:
            data.fill(0)
            # A few silent buffers are expected while a streaming model warms up.
            if self.started_at and time.monotonic() - self.started_at > 2:
                self.output_starved += 1

    def _run(self):
        while not self.stop_event.is_set():
            try:
                block = self.input_queue.get(timeout=0.1)
            except queue.Empty:
                continue
            try:
                output = np.asarray(self.processor.process(block), dtype=np.float32)
                if output.shape != block.shape or not np.isfinite(output).all():
                    raise ValueError("模型输出无效：必须返回与输入等长的有限值音频")
                if self._put_latest(self.output_queue, np.clip(output, -1, 1)):
                    self.output_drops += 1
            except Exception as exc:
                self.stop_event.set()
                self.on_error(str(exc))
                return

    def start(self):
        try:
            input_settings = compatible_audio_settings(self.input_device)
            output_settings = compatible_audio_settings(self.output_device)
            sd.check_input_settings(device=self.input_device, channels=1,
                                    samplerate=self.sample_rate, dtype="float32",
                                    extra_settings=input_settings)
            sd.check_output_settings(device=self.output_device, channels=1,
                                     samplerate=self.sample_rate, dtype="float32",
                                     extra_settings=output_settings)
            self.input_stream = sd.InputStream(
                device=self.input_device, channels=1, samplerate=self.sample_rate,
                blocksize=self.block_size, dtype="float32", callback=self._input_callback,
                extra_settings=input_settings)
            self.output_stream = sd.OutputStream(
                device=self.output_device, channels=1, samplerate=self.sample_rate,
                blocksize=self.block_size, dtype="float32", callback=self._output_callback,
                extra_settings=output_settings)
            self.worker = threading.Thread(target=self._run, daemon=True)
            self.worker.start()
            self.started_at = time.monotonic()
            self.input_stream.start()
            self.output_stream.start()
        except Exception:
            try:
                self.stop()
            except Exception:
                pass  # Preserve the original device error.
            raise

    def stop(self):
        self.stop_event.set()
        errors = []
        for stream in (self.input_stream, self.output_stream):
            if stream is not None:
                try:
                    stream.stop()
                except Exception as exc:
                    errors.append(str(exc))
                try:
                    stream.close()
                except Exception as exc:
                    errors.append(str(exc))
        self.input_stream = self.output_stream = None
        if self.worker and self.worker is not threading.current_thread():
            self.worker.join(timeout=2)
        close = getattr(self.processor, "close", None)
        if self.worker and self.worker.is_alive():
            errors.append("模型处理线程未能及时停止，请稍后重试")
        elif callable(close):
            try:
                close()
            except Exception as exc:
                errors.append(str(exc))
        if errors:
            raise RuntimeError("；".join(errors))
