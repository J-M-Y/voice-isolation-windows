"""Per-session Windows singleton with a wake-up signal for an existing GUI.

The Win32 listener never touches Tk. Call ``take_show_request`` periodically
from the Tk main thread, and restore the existing window there when true.
"""
from __future__ import annotations

import os
import threading


DEFAULT_NAME = r"Local\VoiceIsolation_3E5B771A_Tray_v1"
ERROR_ALREADY_EXISTS = 183
WAIT_OBJECT_0 = 0
INFINITE = 0xFFFFFFFF


class WindowsInstance:
    def __init__(self, name=DEFAULT_NAME, kernel=None):
        self._name = name
        self._kernel = kernel
        self._mutex = None
        self._wake = None
        self._stop = None
        self._thread = None
        self._show = threading.Event()
        self._acquired = False

    @staticmethod
    def _load_kernel():
        import ctypes
        from ctypes import wintypes

        k = ctypes.WinDLL('kernel32', use_last_error=True)
        k.CreateEventW.argtypes = [wintypes.LPVOID, wintypes.BOOL, wintypes.BOOL, wintypes.LPCWSTR]
        k.CreateEventW.restype = wintypes.HANDLE
        k.CreateMutexW.argtypes = [wintypes.LPVOID, wintypes.BOOL, wintypes.LPCWSTR]
        k.CreateMutexW.restype = wintypes.HANDLE
        k.SetEvent.argtypes = [wintypes.HANDLE]
        k.SetEvent.restype = wintypes.BOOL
        k.CloseHandle.argtypes = [wintypes.HANDLE]
        k.CloseHandle.restype = wintypes.BOOL
        k.WaitForMultipleObjects.argtypes = [wintypes.DWORD, ctypes.POINTER(wintypes.HANDLE),
                                            wintypes.BOOL, wintypes.DWORD]
        k.WaitForMultipleObjects.restype = wintypes.DWORD
        return k

    def acquire(self):
        """Return True for the primary instance; otherwise signal it and return False.

        Create the wake event *before* the mutex, so even a concurrent second
        launch cannot signal before the primary's named event exists.
        """
        if self._acquired or self._mutex or self._wake:
            raise RuntimeError('instance guard already started')
        self._show.clear()
        if os.name != 'nt' and self._kernel is None:
            self._acquired = True
            return True
        import ctypes
        from ctypes import wintypes

        k = self._kernel or self._load_kernel()
        self._kernel = k
        try:
            self._wake = k.CreateEventW(None, False, False, self._name + '.wake')
            if not self._wake:
                raise ctypes.WinError(ctypes.get_last_error())
            self._stop = k.CreateEventW(None, True, False, None)
            if not self._stop:
                raise ctypes.WinError(ctypes.get_last_error())
            ctypes.set_last_error(0)
            self._mutex = k.CreateMutexW(None, False, self._name + '.mutex')
            error = ctypes.get_last_error()
            if not self._mutex:
                raise ctypes.WinError(error)
            if error == ERROR_ALREADY_EXISTS:
                if not k.SetEvent(self._wake):
                    raise ctypes.WinError(ctypes.get_last_error())
                self.close()
                return False

            self._acquired = True
            handles = (wintypes.HANDLE * 2)(self._stop, self._wake)

            def listen():
                while True:
                    result = k.WaitForMultipleObjects(2, handles, False, INFINITE)
                    if result == WAIT_OBJECT_0:
                        return
                    if result == WAIT_OBJECT_0 + 1:
                        self._show.set()
                    else:
                        return

            self._thread = threading.Thread(target=listen, name='voice-app-wakeup', daemon=True)
            self._thread.start()
            return True
        except BaseException:
            self.close()
            raise

    def take_show_request(self):
        """Consume a request; invoke this from the Tk main thread only."""
        if self._show.is_set():
            self._show.clear()
            return True
        return False

    def close(self):
        k = self._kernel
        if k is not None and self._stop:
            k.SetEvent(self._stop)
        if self._thread is not None and self._thread is not threading.current_thread():
            self._thread.join(timeout=2)
        # Do not close handles while a listener is still waiting on them.
        if self._thread is None or not self._thread.is_alive():
            if k is not None:
                for handle in (self._mutex, self._wake, self._stop):
                    if handle:
                        k.CloseHandle(handle)
            self._mutex = self._wake = self._stop = None
            self._thread = None
            self._acquired = False
