"""Windows voice isolation prototype, with a replaceable model backend."""
from pathlib import Path
import queue
import threading
import tkinter as tk
from tkinter import filedialog, messagebox, ttk

import numpy as np
import sounddevice as sd

from audio_engine import AudioEngine, com_initialized, compatible_audio_settings, record_reference
from profile_storage import prepare_reference, reference_path
from self_test import run_self_test
from single_instance import WindowsInstance
from tray_control import TrayController
import voice_model


HERE = Path(__file__).resolve().parent
REFERENCE = reference_path()
RATE = 48000
BLOCK = 3840  # 80 ms at 48 kHz


def microphone_test_message(label, level):
    """Give advice only; never gate or change the microphone signal."""
    if level < 0.001:
        return f'测试完成：{label}没有检测到足够的声音，请检查麦克风或换一个设备重试。'
    if level < 0.01:
        return (f'测试完成：{label}检测到声音，但音量偏小。建议靠近麦克风或调大系统麦克风输入，'
                '并让播放视频的设备离麦克风远一些；你仍然可以继续录音。')
    return f'测试完成：{label}检测到声音，音量可以用来录入。'


def process_comparison(audio, processor):
    """Process a short recording through the same streaming path as live audio."""
    raw = np.asarray(audio, dtype=np.float32).reshape(-1)
    if not raw.size or not np.isfinite(raw).all():
        raise ValueError("测试录音无效，请检查麦克风。")
    processed = []
    for offset in range(0, len(raw), BLOCK):
        block = raw[offset:offset + BLOCK]
        n = len(block)
        if n < BLOCK:
            block = np.pad(block, (0, BLOCK - n))
        output = np.asarray(processor.process(block), dtype=np.float32)
        if output.shape != (BLOCK,) or not np.isfinite(output).all():
            raise ValueError("处理后的测试音频无效。")
        processed.append(output[:n])
    return np.clip(np.concatenate(processed), -1, 1)


def device_choices(devices, hosts, direction):
    """List one friendly Windows endpoint per device, preferring WASAPI."""
    channel = "max_input_channels" if direction == "input" else "max_output_channels"
    host_names = {hosts[d['hostapi']]['name'] for d in devices if d[channel]}
    preferred = next((h for h in ('Windows WASAPI', 'Windows DirectSound', 'MME')
                      if h in host_names), None)
    candidates = [(i, d) for i, d in enumerate(devices)
                  if d[channel] and hosts[d['hostapi']]['name'] == preferred]
    result = {}
    for index, d in candidates:
        name = d['name'].strip()
        folded = name.casefold()
        # The recording end of virtual cables and loopback devices is not a physical mic.
        if direction == 'input' and any(x in folded for x in (
                'cable output', 'vb-audio', 'voicemeeter', 'stereo mix',
                '立体声混音', 'sonar - microphone', 'crystalsound', 'loopback')):
            continue
        if direction == 'output' and ('cable in 16ch' in folded or 'microsoft' in folded):
            continue
        if direction == 'input' and 'web camera' in folded:
            label = '摄像头麦克风（Web Camera）'
        elif direction == 'input' and 'realtek' in folded:
            label = '电脑／耳机插孔麦克风（Realtek）'
        elif direction == 'output' and 'cable input' in folded:
            label = '虚拟麦克风（CABLE Input，推荐）'
        else:
            label = name
        unique = label
        n = 2
        while unique in result:
            unique = f'{label}（{n}）'
            n += 1
        result[unique] = index
    return result


class VoiceApp:
    def __init__(self, root, instance_guard=None):
        self.root = root
        self.root.title("指定人声提取 · 实验版")
        self.root.geometry("540x630")
        self.root.minsize(490, 540)
        # Older portable builds saved the recording next to app.py or in
        # PyInstaller's _internal directory. Only inspect that adjacent path;
        # never look for another person's recordings or modify the old file.
        try:
            prepare_reference(REFERENCE, (HERE / "我的声样.wav", HERE.parent / "我的声样.wav"))
        except OSError as exc:
            messagebox.showwarning("声样目录不可用", f"无法准备本地声样目录：{exc}\n请检查当前用户的文件夹权限。")
        self.events = queue.Queue()
        self.instance_guard = instance_guard
        self.tray = None
        self._comparison_hidden = False
        self.engine = None
        self.busy = False
        self.closing = False
        self.pending_stream_error = None
        self.run_generation = 0
        self.inputs = {}
        self.outputs = {}
        self.input_var = tk.StringVar()
        self.output_var = tk.StringVar()
        self.output_info = tk.StringVar()
        self.tray_hint_var = tk.StringVar(value="关闭窗口后可在托盘继续运行")
        self.show_output_options = tk.BooleanVar(value=False)
        self.strong_music_var = tk.BooleanVar(value=True)
        self.reference_var = tk.StringVar(value=str(REFERENCE) if REFERENCE.exists() else "尚未录制")
        self.status_var = tk.StringVar(value="准备就绪。请先选择设备并录制声样。")
        self.comparison_window = None
        self.comparison_raw = None
        self.comparison_processed = None
        self.build_ui()
        self.refresh_devices()
        self.tray = TrayController(self.events)
        if not self.tray.start():
            self.tray_hint_var.set("托盘不可用，关闭窗口将退出")
            self.status_var.set("系统托盘不可用，程序仍可正常在窗口中使用。")
        self.root.bind("<Unmap>", self._on_unmap, add="+")
        self.root.protocol("WM_DELETE_WINDOW", self.close)
        self.root.after(100, self.poll)

    def build_ui(self):
        frame = ttk.Frame(self.root, padding=12)
        frame.pack(fill="both", expand=True)
        heading = ttk.Frame(frame)
        heading.pack(fill="x", pady=(0, 4))
        ttk.Label(heading, text="人声提取", font=("Microsoft YaHei UI", 15, "bold")).pack(side="left")
        ttk.Button(heading, text="退出", command=lambda: self.quit_app()).pack(side="right", padx=(8, 0))
        ttk.Label(heading, textvariable=self.tray_hint_var, foreground="#536779").pack(side="right")

        device_box = ttk.LabelFrame(frame, text="1. 麦克风", padding=8)
        device_box.pack(fill="x", pady=4)
        self.input_combo = ttk.Combobox(device_box, textvariable=self.input_var, state="readonly")
        self.input_combo.pack(fill="x", pady=(0, 4))
        device_actions = ttk.Frame(device_box)
        device_actions.pack(fill="x", pady=(0, 2))
        ttk.Button(device_actions, text="测试麦克风", command=self.test_microphone).pack(side="left", padx=(0, 5))
        ttk.Button(device_actions, text="刷新设备", command=lambda: self.refresh_devices(reinitialize=True)).pack(side="left")
        ttk.Checkbutton(device_actions, text="输出设备（高级）", variable=self.show_output_options,
                        command=self.toggle_output_options).pack(side="right")
        output_label = ttk.Label(device_box, textvariable=self.output_info, wraplength=470)
        output_label.pack(fill="x", pady=(2, 0))
        self.output_combo = ttk.Combobox(device_box, textvariable=self.output_var, state="readonly")
        self.output_combo.bind('<<ComboboxSelected>>', lambda event: self.update_output_info())

        ref_box = ttk.LabelFrame(frame, text="2. 你的声音", padding=8)
        ref_box.pack(fill="x", pady=4)
        self.reference_display = tk.StringVar()
        def update_reference_display(*_args):
            value = self.reference_var.get()
            self.reference_display.set("声样：已录入" if Path(value).is_file() else "声样：尚未录入")
        self.reference_var.trace_add("write", update_reference_display)
        update_reference_display()
        buttons = ttk.Frame(ref_box)
        buttons.pack(fill="x")
        self.record_button = ttk.Button(buttons, text="录入声音（12 秒）", command=self.record)
        self.record_button.pack(side="left", padx=(0, 5))
        ttk.Button(buttons, text="选择已有声样", command=self.select_reference).pack(side="left", padx=(0, 8))
        ttk.Label(buttons, textvariable=self.reference_display).pack(side="left")
        ttk.Label(ref_box, text="录入时请在安静环境中连续说话。", foreground="#536779").pack(anchor="w", pady=(3, 0))

        run_box = ttk.LabelFrame(frame, text="3. 人声处理", padding=8)
        run_box.pack(fill="x", pady=4)
        ttk.Checkbutton(run_box, text="加强背景音乐过滤（下次启动或试听时生效）",
                        variable=self.strong_music_var).pack(anchor="w")
        buttons = ttk.Frame(run_box)
        buttons.pack(fill="x", pady=(5, 0))
        self.start_button = ttk.Button(buttons, text="开始处理", command=self.start)
        self.start_button.pack(side="left", padx=(0, 5))
        self.stop_button = ttk.Button(buttons, text="停止", command=self.stop, state="disabled")
        self.stop_button.pack(side="left", padx=(0, 5))
        ttk.Button(buttons, text="试听前后效果", command=self.open_comparison).pack(side="left")
        ttk.Label(run_box, text="游戏或语音软件的麦克风请选择 CABLE Output。",
                  foreground="#536779").pack(anchor="w", pady=(5, 0))
        status = ttk.Label(frame, textvariable=self.status_var, wraplength=480, foreground="#174b80")
        status.pack(fill="x", pady=(7, 0))
        # Keep long diagnostics inside the window when the user resizes it.
        for label in (output_label, status):
            label.bind("<Configure>", lambda event: event.widget.configure(wraplength=max(event.width, 1)))

    def open_comparison(self):
        if self.busy or self.engine:
            messagebox.showinfo("请先停止处理", "请先点击主窗口的“停止”，再录制试听。")
            return
        try:
            self.selected_input()
            if not Path(self.reference_var.get()).is_file():
                raise ValueError("请先录入 12 秒声样，再进行降噪对比。")
        except ValueError as exc:
            messagebox.showerror("无法试听", str(exc))
            return
        if self.comparison_window is not None and self.comparison_window.winfo_exists():
            self.comparison_window.lift()
            return
        physical = {name: index for name, index in self.outputs.items()
                    if not any(v in sd.query_devices(index)['name'].casefold() for v in
                               ('cable', 'vb-audio', 'voicemeeter', 'sonar', 'crystalsound', 'virtual'))}
        if not physical:
            messagebox.showerror("无法试听", "没有检测到实体耳机或扬声器，请先连接耳机并刷新设备。")
            return
        self.comparison_outputs = physical
        default_out = sd.default.device[1]
        default_name = sd.query_devices(default_out)['name'].casefold() if default_out is not None and default_out >= 0 else ''
        first = next((name for name, i in physical.items()
                      if sd.query_devices(i)['name'].casefold() == default_name), next(iter(physical)))
        self.comparison_output_var = tk.StringVar(value=first)
        self.comparison_raw = self.comparison_processed = None
        self.comparison_window = tk.Toplevel(self.root)
        self.comparison_window.title("试听降噪前后")
        self.comparison_window.geometry("600x330")
        frame = ttk.Frame(self.comparison_window, padding=18)
        frame.pack(fill="both", expand=True)
        ttk.Label(frame, text="录制 5 秒并试听", font=("Microsoft YaHei UI", 15, "bold")).pack(anchor="w")
        ttk.Label(frame, text="请戴耳机。录制时先只播放背景声约 2 秒，再说话约 3 秒；分别听原声和处理后，注意突发背景声和自己的声音。录音只保存在内存；实时通话还需启动处理并在游戏中选 CABLE Output。", wraplength=550).pack(anchor="w", pady=10)
        ttk.Label(frame, text="试听设备：选择你的实体耳机或扬声器").pack(anchor="w")
        self.comparison_output_combo = ttk.Combobox(frame, textvariable=self.comparison_output_var,
                                                     values=list(physical), state="readonly")
        self.comparison_output_combo.pack(fill="x", pady=(2, 8))
        row = ttk.Frame(frame)
        row.pack(anchor="w")
        ttk.Button(row, text="开始录制 5 秒", command=self.record_comparison).pack(side="left", padx=(0, 6))
        self.raw_button = ttk.Button(row, text="播放原声", command=lambda: self.play_comparison(False), state="disabled")
        self.raw_button.pack(side="left", padx=(0, 6))
        self.processed_button = ttk.Button(row, text="播放处理后", command=lambda: self.play_comparison(True), state="disabled")
        self.processed_button.pack(side="left")
        self.comparison_status = tk.StringVar(value="准备好后点击录制。")
        ttk.Label(frame, textvariable=self.comparison_status, wraplength=550, foreground="#174b80").pack(anchor="w", pady=(12, 0))

    def record_comparison(self):
        if self.busy or self.engine:
            messagebox.showinfo("请稍候", "请先停止实时处理或等待当前操作完成。")
            return
        try:
            inp = self.selected_input()
            ref = Path(self.reference_var.get())
            if not ref.is_file():
                raise ValueError("声样文件不存在，请先录入 12 秒声样。")
        except ValueError as exc:
            messagebox.showerror("无法录制", str(exc))
            return
        self.busy = True
        self.raw_button.configure(state="disabled")
        self.processed_button.configure(state="disabled")
        self.comparison_status.set("正在录制 5 秒，请播放音乐并说话……")
        strong_music = self.strong_music_var.get()

        def work():
            processor = None
            try:
                with com_initialized():
                    raw = sd.rec(5 * RATE, samplerate=RATE, channels=1,
                                 dtype="float32", device=inp, blocking=True,
                                 extra_settings=compatible_audio_settings(inp)).reshape(-1)
                processor = voice_model.create_processor(str(ref), RATE, BLOCK, strong_music=strong_music)
                result = process_comparison(raw, processor)
                self.events.put(("comparison_ready", (raw, result)))
            except Exception as exc:
                self.events.put(("comparison_failed", f"试听录制失败：{exc}"))
            finally:
                if processor is not None:
                    processor.close()
        threading.Thread(target=work, daemon=True).start()

    def play_comparison(self, processed):
        if self.busy or self.engine:
            messagebox.showinfo("请稍候", "请先停止实时处理或等待当前操作完成。")
            return
        data = self.comparison_processed if processed else self.comparison_raw
        output = self.comparison_outputs.get(self.comparison_output_var.get())
        if data is None or output is None:
            messagebox.showerror("无法播放", "请先录制，并选择实体耳机或扬声器。")
            return
        self.busy = True
        self.comparison_status.set("正在播放处理后的声音……" if processed else "正在播放原声……")

        def work():
            try:
                with com_initialized():
                    sd.play(data, samplerate=RATE, device=output, blocking=True,
                            extra_settings=compatible_audio_settings(output))
                self.events.put(("comparison_played", None))
            except Exception as exc:
                self.events.put(("comparison_failed", f"试听播放失败：{exc}"))
        threading.Thread(target=work, daemon=True).start()

    def toggle_output_options(self):
        if self.show_output_options.get():
            self.output_combo.pack(fill="x", pady=(2, 6))
        else:
            self.output_combo.pack_forget()

    def update_output_info(self):
        selected = self.output_var.get()
        if 'cable input' in selected.casefold():
            self.output_info.set('虚拟声卡：已自动选好（CABLE Input）。')
        elif selected:
            self.output_info.set('已选择其他输出设备。游戏麦克风需要连接虚拟声卡。')
        else:
            self.output_info.set('未找到虚拟声卡：安装版可从开始菜单“人声提取”打开“VB-CABLE 驱动安装程序”；便携版请打开文件夹中的 VB-CABLE\\driver\\VBCABLE_Setup_x64.exe。安装需要管理员权限，完成后请重启电脑。')

    def refresh_devices(self, reinitialize=False):
        if self.busy or self.engine:
            return
        comparison = getattr(self, 'comparison_window', None)
        if reinitialize and comparison is not None and comparison.winfo_exists():
            # The comparison dialog holds PortAudio device indexes. Reloading
            # devices underneath it could make those indexes refer to a
            # different output, including a virtual cable.
            messagebox.showinfo("请先关闭试听窗口", "请关闭试听窗口后再刷新设备。")
            return
        try:
            if reinitialize:
                # PortAudio caches endpoints at initialization. Reinitialize
                # only while no app stream is active so a newly connected
                # headset or virtual cable appears without a full restart.
                sd._terminate()
                sd._initialize()
            devices, hosts = sd.query_devices(), sd.query_hostapis()
            self.inputs = device_choices(devices, hosts, 'input')
            self.outputs = device_choices(devices, hosts, 'output')
            self.input_combo["values"] = list(self.inputs)
            self.output_combo["values"] = list(self.outputs)
            default_input = sd.default.device[0]
            if self.input_var.get() not in self.inputs:
                default_name = devices[default_input]['name'].strip().casefold() if 0 <= default_input < len(devices) else ''
                self.input_var.set(next((k for k, v in self.inputs.items()
                                         if devices[v]['name'].strip().casefold() == default_name),
                                        next(iter(self.inputs), '')))
            if self.output_var.get() not in self.outputs:
                # Do not silently select speakers; feeding audio into speakers may cause feedback.
                self.output_var.set(next((k for k in self.outputs if "cable input" in k.lower()), ""))
            self.update_output_info()
            if reinitialize:
                self.status_var.set("设备列表已刷新。")
        except Exception as exc:
            messagebox.showerror("设备读取失败", f"{exc}\n请关闭软件后重新打开并检查设备。")

    def selected_input(self):
        if self.input_var.get() not in self.inputs:
            raise ValueError("请选择麦克风输入设备。")
        return self.inputs[self.input_var.get()]

    def test_microphone(self):
        if self.busy or self.engine:
            return
        try:
            device = self.selected_input()
        except ValueError as exc:
            messagebox.showerror('不能测试', str(exc))
            return
        label = self.input_var.get()
        self.busy = True
        self.status_var.set('正在测试：请对着麦克风正常说话，持续 3 秒……')

        def work():
            try:
                with com_initialized():
                    audio = sd.rec(3 * RATE, samplerate=RATE, channels=1,
                                   dtype='float32', device=device, blocking=True,
                                   extra_settings=compatible_audio_settings(device))
                level = float(np.sqrt(np.mean(np.square(audio.astype(np.float64)))))
                if not np.isfinite(level):
                    raise ValueError('采集到无效音频，请检查麦克风。')
                self.events.put(('mic_test', (label, level)))
            except Exception as exc:
                self.events.put(('failed', f'测试失败：{exc}'))
        threading.Thread(target=work, daemon=True).start()

    def record(self):
        if self.busy or self.engine:
            return
        try:
            device = self.selected_input()
        except ValueError as exc:
            messagebox.showerror("不能录音", str(exc))
            return
        self.busy = True
        self.status_var.set("正在录制 12 秒，请以正常音量连续说话……")

        def work():
            temp = REFERENCE.with_name("声样录制中.tmp.wav")
            try:
                # Record to a temporary path so a failed capture does not destroy the old reference.
                with com_initialized():
                    record_reference(device, RATE, 12, temp)
                temp.replace(REFERENCE)
                self.events.put(("recorded", str(REFERENCE)))
            except Exception as exc:
                try:
                    temp.unlink(missing_ok=True)
                except OSError:
                    pass
                self.events.put(("failed", f"录音失败：{exc}"))
        threading.Thread(target=work, daemon=True).start()

    def select_reference(self):
        if self.busy or self.engine:
            return
        path = filedialog.askopenfilename(title="选择声样", filetypes=[("WAV 音频", "*.wav")])
        if path:
            self.reference_var.set(path)

    def start(self):
        if self.busy or self.engine:
            return
        try:
            inp = self.selected_input()
            out = self.outputs.get(self.output_var.get())
            if out is None:
                raise ValueError("未找到虚拟声卡。安装版请从开始菜单“人声提取”打开“VB-CABLE 驱动安装程序”；便携版请打开文件夹中的 VB-CABLE\\driver\\VBCABLE_Setup_x64.exe。完成安装并重启电脑后重试。")
            ref = Path(self.reference_var.get())
            if not ref.is_file():
                raise ValueError("请先录入声音或选择声样 WAV 文件。")
        except ValueError as exc:
            messagebox.showerror("无法启动", str(exc))
            return
        self.busy = True
        self.run_generation += 1
        generation = self.run_generation
        self.pending_stream_error = None
        self.status_var.set("正在加载人声模型和音频设备，请稍候……")
        strong_music = self.strong_music_var.get()

        def work():
            try:
                processor = voice_model.create_processor(str(ref), RATE, BLOCK, strong_music=strong_music)
                engine = AudioEngine(inp, out, processor, sample_rate=RATE, block_size=BLOCK,
                                     on_error=lambda err: self.events.put(("stream_error", (generation, err))))
                with com_initialized():
                    engine.start()
                self.events.put(("started", (generation, engine)))
            except Exception as exc:
                self.events.put(("start_failed", (generation, f"启动失败：{exc}")))
        threading.Thread(target=work, daemon=True).start()

    def stop(self):
        self.run_generation += 1
        self.pending_stream_error = None
        if self.engine:
            try:
                self.engine.stop()
            except Exception as exc:
                self.show_window()
                messagebox.showwarning("停止时出错", str(exc))
            self.engine = None
        tray = getattr(self, 'tray', None)
        if tray:
            tray.set_running(False)
        self.start_button.configure(state="normal")
        self.stop_button.configure(state="disabled")
        self.status_var.set("已停止。")

    def poll(self):
        guard = getattr(self, 'instance_guard', None)
        if guard and guard.take_show_request():
            self.show_window()
        try:
            while True:
                kind, payload = self.events.get_nowait()
                if kind == 'tray_show':
                    self.show_window()
                elif kind == 'tray_exit':
                    if self.quit_app():
                        return
                elif kind == 'tray_failed':
                    self.show_window()
                    self.tray_hint_var.set('托盘不可用，关闭窗口将退出')
                    self.status_var.set('系统托盘不可用，窗口已恢复。')
                    messagebox.showwarning('系统托盘不可用', f'后台图标已失效，窗口已恢复。\n{payload}')
                elif kind == "recorded":
                    self.busy = False
                    self.reference_var.set(payload)
                    self.status_var.set("录制完成，可以启动人声提取。")
                elif kind == 'mic_test':
                    self.busy = False
                    label, level = payload
                    self.status_var.set(microphone_test_message(label, level))
                elif kind == 'comparison_ready':
                    self.busy = False
                    self.comparison_raw, self.comparison_processed = payload
                    if self.comparison_window is not None and self.comparison_window.winfo_exists():
                        self.raw_button.configure(state='normal')
                        self.processed_button.configure(state='normal')
                        self.comparison_status.set('录制完成。请分别点“播放原声”和“播放处理后”比较。')
                elif kind == 'comparison_played':
                    self.busy = False
                    if self.comparison_window is not None and self.comparison_window.winfo_exists():
                        self.comparison_status.set('播放完成。可以继续试听另一段。')
                elif kind == 'comparison_failed':
                    self.busy = False
                    if self.comparison_window is not None and self.comparison_window.winfo_exists():
                        self.comparison_status.set(payload)
                    self.show_window()
                    messagebox.showerror('试听出错', payload)
                elif kind == "started":
                    generation, payload = payload
                    if generation != self.run_generation:
                        payload.stop()
                        continue
                    self.busy = False
                    self.engine = payload
                    self.start_button.configure(state="disabled")
                    self.stop_button.configure(state="normal")
                    tray = getattr(self, 'tray', None)
                    if tray:
                        tray.set_running(True)
                    self.status_var.set("正在处理。请在游戏里选择虚拟声卡的录音端。")
                    if self.pending_stream_error:
                        error, self.pending_stream_error = self.pending_stream_error, None
                        self.stop()
                        self.show_window()
                        messagebox.showerror("处理出错", error)
                elif kind == "stream_error":
                    generation, payload = payload
                    if generation != self.run_generation:
                        continue
                    if self.busy and not self.engine:
                        self.pending_stream_error = payload
                    else:
                        self.stop()
                        self.show_window()
                        messagebox.showerror("处理出错", payload)
                elif kind == "failed":
                    self.busy = False
                    self.status_var.set(payload)
                    self.show_window()
                    messagebox.showerror("操作失败", payload)
                elif kind == "start_failed":
                    generation, payload = payload
                    if generation == self.run_generation:
                        self.busy = False
                        self.pending_stream_error = None
                        self.status_var.set(payload)
                        self.show_window()
                        messagebox.showerror("操作失败", payload)
        except queue.Empty:
            pass
        if self.engine:
            e = self.engine
            if e.input_drops >= 3 or e.output_drops >= 3 or e.output_starved >= 10 or e.callback_problems >= 3:
                self.status_var.set(
                    "警告：处理或音频设备可能跟不上，出现缺帧，声音可能断续。"
                    f" 输入丢帧 {e.input_drops}，输出丢帧 {e.output_drops}，"
                    f"输出缺帧 {e.output_starved}，设备告警 {e.callback_problems}。"
                    "请关闭占用资源的程序或更换设备后重试。")
        if not self.closing:
            self.root.after(100, self.poll)

    def _on_unmap(self, event):
        # Tk also sends Unmap for child widgets and for our own withdraw().
        if event.widget is self.root and not self.closing:
            self.root.after(80, self._hide_if_minimized)

    def _hide_if_minimized(self):
        if not self.closing and self.root.winfo_exists() and self.root.state() == 'iconic':
            tray = getattr(self, 'tray', None)
            if tray and tray.available:
                self.hide_to_tray()

    def hide_to_tray(self):
        tray = getattr(self, 'tray', None)
        if not tray or not tray.available:
            return False
        comparison = getattr(self, 'comparison_window', None)
        self._comparison_hidden = bool(comparison and comparison.winfo_exists() and comparison.winfo_viewable())
        if self._comparison_hidden:
            comparison.withdraw()
        self.root.withdraw()
        return True

    def show_window(self):
        self.root.deiconify()
        self.root.lift()
        comparison = getattr(self, 'comparison_window', None)
        if getattr(self, '_comparison_hidden', False) and comparison and comparison.winfo_exists():
            comparison.deiconify()
            comparison.lift()
        self._comparison_hidden = False

    def close(self):
        # A normal close keeps active microphone processing running in tray.
        if not self.hide_to_tray():
            self.quit_app()

    def quit_app(self):
        if self.busy:
            self.show_window()
            messagebox.showinfo("请稍候", "录音或初始化仍在进行，请稍候再退出。")
            return False
        if self.closing:
            return True
        self.closing = True
        self.stop()
        tray = getattr(self, 'tray', None)
        if tray:
            tray.stop()
        guard = getattr(self, 'instance_guard', None)
        if guard:
            guard.close()
        self.root.destroy()
        return True


if __name__ == "__main__":
    import sys
    if len(sys.argv) == 4 and sys.argv[1] == '--self-test':
        raise SystemExit(run_self_test(sys.argv[2], sys.argv[3]))
    if '--self-test' in sys.argv:
        raise SystemExit(2)
    guard = WindowsInstance()
    try:
        if guard.acquire():
            root = tk.Tk()
            VoiceApp(root, instance_guard=guard)
            root.mainloop()
    finally:
        guard.close()
