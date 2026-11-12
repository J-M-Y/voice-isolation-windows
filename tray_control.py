"""Windows notification-area integration, with no Tk calls outside the UI thread.

The controller only sends messages to a thread-safe queue. The caller is
responsible for consuming them on the Tk thread and for not hiding a window
unless start() returned True.
"""
from __future__ import annotations

import threading


def _make_tray_icon(on_show, on_exit, on_failure):
    """Build a Windows icon. Imports are lazy so errors can be shown to users."""
    import ctypes
    import pystray
    from PIL import Image, ImageDraw
    from pystray._win32 import Icon as WindowsIcon, win32

    # pystray's Windows backend ignores Shell_NotifyIcon's return value. When
    # Explorer is unavailable, that would make the app believe it can be
    # recovered after hiding. Detect NIM_ADD failures before reporting readiness.
    class CheckedWindowsIcon(WindowsIcon):
        def _show(self):
            try:
                super()._show()
            except Exception as exc:
                # This also runs when Explorer restarts and re-registers icons.
                # Tell the UI to reveal itself if re-registration fails.
                on_failure(str(exc))
                raise

        def _message(self, code, flags, **kwargs):
            ok = win32.Shell_NotifyIcon(code, win32.NOTIFYICONDATAW(
                cbSize=ctypes.sizeof(win32.NOTIFYICONDATAW),
                hWnd=self._hwnd,
                hID=id(self),
                uFlags=flags,
                **kwargs,
            ))
            if code == win32.NIM_ADD and not ok:
                raise OSError('Windows 没有接受系统托盘图标。')

    size = 64
    image = Image.new('RGBA', (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle((2, 2, 61, 61), radius=15, fill='#2864d8')
    draw.rounded_rectangle((25, 12, 38, 37), radius=7, fill='white')
    draw.arc((19, 21, 44, 45), 0, 180, fill='white', width=4)
    draw.line((31, 44, 31, 51), fill='white', width=4)
    draw.line((23, 52, 40, 52), fill='white', width=4)

    menu = pystray.Menu(
        pystray.MenuItem('打开窗口', lambda icon, item: on_show(), default=True),
        pystray.MenuItem('退出软件', lambda icon, item: on_exit()),
    )
    return CheckedWindowsIcon('VoiceIsolation', image, '人声提取（后台运行）', menu)


class TrayController:
    """Own a notification icon and post show/exit events to a queue."""

    def __init__(self, events, icon_factory=None):
        self.events = events
        self._icon_factory = icon_factory or _make_tray_icon
        self._icon = None
        self._thread = None
        self._ready = threading.Event()
        self._failed = threading.Event()
        self._stopping = threading.Event()
        self.available = False
        self.error = ''
        self._processing = False

    def _post_show(self):
        self.events.put(('tray_show', None))

    def _post_exit(self):
        self.events.put(('tray_exit', None))

    def _post_failure(self, error):
        if self.available and not self._stopping.is_set():
            self.available = False
            self.error = error
            self.events.put(('tray_failed', error))

    def _setup(self, icon):
        try:
            if self._failed.is_set() or self._stopping.is_set():
                return
            icon.title = self._title()
            icon.visible = True
            self.available = True
        except Exception as exc:
            self.error = str(exc)
            self._failed.set()
        finally:
            self._ready.set()

    def _run(self):
        try:
            self._icon.run(setup=self._setup)
        except Exception as exc:
            self.error = str(exc)
            self._failed.set()
            # pystray creates a setup thread that waits for _mark_ready. If its
            # Windows loop fails before then, release that thread so app exit
            # is never held open by a failed tray startup.
            if not self._ready.is_set():
                try:
                    self._icon._mark_ready()
                except Exception:
                    pass
            self._ready.set()
        finally:
            had_icon = self.available
            self.available = False
            if had_icon and not self._stopping.is_set():
                self.events.put(('tray_failed', self.error or '系统托盘意外退出。'))

    def start(self, timeout=4.0):
        """Start icon and return True only when its registration succeeded."""
        if self.available:
            return True
        if self._thread and self._thread.is_alive():
            return False
        self._ready.clear()
        self._failed.clear()
        self._stopping.clear()
        self.error = ''
        try:
            self._icon = self._icon_factory(self._post_show, self._post_exit, self._post_failure)
            self._thread = threading.Thread(target=self._run, name='voice-tray', daemon=True)
            self._thread.start()
            if not self._ready.wait(timeout):
                self.error = '系统托盘启动超时。'
                self._failed.set()
                # Release pystray's non-daemon setup thread if its native loop
                # could not announce readiness. The window remains visible.
                try:
                    self._icon._mark_ready()
                except Exception:
                    pass
                self.stop()
                return False
            ok = self.available and not self._failed.is_set()
            if not ok:
                self.stop()
            return ok
        except Exception as exc:
            self.error = str(exc)
            return False

    def _title(self):
        return '人声提取：正在过滤' if self._processing else '人声提取：已停止'

    def set_running(self, running):
        """Update the hover text; it is safe even if the shell has gone away."""
        self._processing = bool(running)
        if self._icon is not None:
            try:
                self._icon.title = self._title()
            except Exception:
                # A notification-area failure must never stop microphone audio.
                pass

    def stop(self):
        """Remove the icon and stop its message loop; safe to call repeatedly."""
        self._stopping.set()
        icon = self._icon
        if icon is not None:
            try:
                icon.stop()
            except Exception:
                pass
        thread = self._thread
        if thread and thread is not threading.current_thread():
            thread.join(timeout=2)
        self.available = False
