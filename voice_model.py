"""Real-time enrolled-speaker extraction using VoxLock.

The two FIR resamplers are stateful: processing 80 ms blocks does not reset
their phase/filter history at every block boundary. No microphone audio is
passed through before the extractor is ready.
"""

import numpy as np
from scipy import signal
import sherpa_onnx
import voxlock
from pathlib import Path


class VoiceProcessor:
    def __init__(self, reference_path, sample_rate, block_size, strong_music=False):
        if sample_rate != 48000 or block_size % 384:
            raise ValueError("当前模型要求 48000 Hz 和 384 的整数倍帧长")
        ref = voxlock.load_audio(reference_path)
        if len(ref) < 3 * 16000:
            raise ValueError("声样太短，请至少录制 3 秒的声音")
        if np.sqrt(np.mean(ref.astype(np.float64) ** 2)) < 0.001:
            raise ValueError("声样音量过低，请检查麦克风后重新录制")
        self.block_size = block_size
        self.strong_music = bool(strong_music)
        model_dir = Path(__file__).resolve().parent / "models"
        denoiser_name = "dpdfnet2.onnx" if self.strong_music else "gtcrn_simple.onnx"
        if not all((model_dir / n).is_file() for n in ("enroll.onnx", "chunk_int8.onnx", denoiser_name)):
            raise FileNotFoundError("模型文件不完整：请先运行 安装模型.bat")
        # Both modes preserve the extractor-first ordering. The optional
        # stronger mode uses a different denoiser after speaker extraction;
        # placing it before extraction loses short speech in our tests.
        denoiser_model = (
            sherpa_onnx.OfflineSpeechDenoiserModelConfig(
                dpdfnet=sherpa_onnx.OfflineSpeechDenoiserDpdfNetModelConfig(
                    model=str(model_dir / denoiser_name)),
                num_threads=1, provider="cpu", debug=False)
            if self.strong_music else
            sherpa_onnx.OfflineSpeechDenoiserModelConfig(
                gtcrn=sherpa_onnx.OfflineSpeechDenoiserGtcrnModelConfig(
                    model=str(model_dir / denoiser_name)),
                num_threads=1, provider="cpu", debug=False)
        )
        config = sherpa_onnx.OnlineSpeechDenoiserConfig(
            model=denoiser_model)
        if not config.validate():
            raise RuntimeError("降噪模型配置无效，请重新安装模型")
        self.denoiser = sherpa_onnx.OnlineSpeechDenoiser(config)
        if self.denoiser.sample_rate != 16000:
            raise RuntimeError("降噪模型采样率不匹配")
        # A streaming denoiser emits variable-sized chunks. The initial
        # silence covers its frame shift without discarding delayed audio.
        self.denoise_queue = np.zeros(self.denoiser.frame_shift_in_samples, dtype=np.float32)
        self.vl = voxlock.VoxLock(model_dir=model_dir, threads=1)
        self.stream = self.vl.stream(ref)
        # 48 -> 16 kHz low-pass, then 16 -> 48 kHz reconstruction.
        self.kernel = signal.firwin(97, 0.30).astype(np.float32)
        self.down_state = np.zeros(96, dtype=np.float32)
        self.up_state = np.zeros(96, dtype=np.float32)
        # VoxLock holds some samples for its STFT; an initial 512-sample
        # cushion preserves stream order and absorbs that variable output.
        self.queue = np.zeros(512, dtype=np.float32)

    def _extract(self, samples):
        ready = np.asarray(self.stream.push(np.ascontiguousarray(samples)), dtype=np.float32)
        self.queue = np.concatenate((self.queue, ready))
        if len(self.queue) < len(samples):
            # Maintain monotonic order during an exceptional short model
            # output; insert silence before the queued audio rather than
            # leaking the unprocessed microphone input.
            self.queue = np.pad(self.queue, (len(samples) - len(self.queue), 0))
        selected, self.queue = self.queue[:len(samples)], self.queue[len(samples):]
        return selected

    def _denoise(self, samples):
        denoised = np.asarray(self.denoiser(np.ascontiguousarray(samples), 16000).samples, dtype=np.float32)
        self.denoise_queue = np.concatenate((self.denoise_queue, denoised))
        if len(self.denoise_queue) < len(samples):
            # Preserve any delayed speech in order and insert silence rather
            # than passing unprocessed sound through.
            self.denoise_queue = np.pad(self.denoise_queue, (len(samples) - len(self.denoise_queue), 0))
        selected, self.denoise_queue = self.denoise_queue[:len(samples)], self.denoise_queue[len(samples):]
        return selected

    def _process_16k(self, samples):
        return self._denoise(self._extract(samples))

    def process(self, block):
        x = np.asarray(block, dtype=np.float32)
        if x.ndim != 1 or len(x) != self.block_size:
            raise ValueError("输入音频块大小不匹配")
        filtered, self.down_state = signal.lfilter(
            self.kernel, [1.0], x, zi=self.down_state)
        down = filtered[::3].astype(np.float32)
        selected = self._process_16k(down)
        up = np.zeros(len(x), dtype=np.float32)
        up[::3] = selected * 3
        up, self.up_state = signal.lfilter(self.kernel, [1.0], up, zi=self.up_state)
        return np.asarray(up, dtype=np.float32)

    def close(self):
        pass


def create_processor(reference_path, sample_rate, block_size, strong_music=False):
    return VoiceProcessor(reference_path, sample_rate, block_size, strong_music=strong_music)
