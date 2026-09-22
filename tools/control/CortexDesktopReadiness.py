"""Windows-only, source-independent evidence for the *window-visible* startup stage.

This is NOT provider, conversation, agent or complete UI certification. The
project-owned PCC remains the launcher and authority; this module only observes
its already-spawned Cortex Desktop process. It performs no filesystem writes,
window manipulation or child-process termination.
"""
from __future__ import annotations

import sys
import time
from dataclasses import dataclass
from typing import Callable


@dataclass(frozen=True)
class DesktopWindowObservation:
    status: str
    pid: int
    detail: str
    window_handle: int | None = None
    exit_code: int | None = None


def find_visible_window(pid: int) -> int | None:
    """Return a visible top-level window of exactly this PID, not another app.

    Never use a window title alone to identify Cortex, and do not infer that a
    displayed window means the backend/model/chat has finished initializing.
    """
    if sys.platform != "win32":
        raise RuntimeError("native Win32 window inspection is unavailable on this host")

    import ctypes
    from ctypes import wintypes

    user32 = ctypes.WinDLL("user32", use_last_error=True)
    callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    user32.EnumWindows.argtypes = [callback_type, wintypes.LPARAM]
    user32.EnumWindows.restype = wintypes.BOOL
    user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user32.GetWindowThreadProcessId.restype = wintypes.DWORD
    user32.IsWindowVisible.argtypes = [wintypes.HWND]
    user32.IsWindowVisible.restype = wintypes.BOOL
    user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
    user32.GetWindowRect.restype = wintypes.BOOL

    matches: list[int] = []

    @callback_type
    def visit(handle: int, unused: int) -> bool:
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(handle, ctypes.byref(owner))
        if owner.value != pid or not user32.IsWindowVisible(handle):
            return True
        bounds = wintypes.RECT()
        if not user32.GetWindowRect(handle, ctypes.byref(bounds)):
            return True
        if bounds.right - bounds.left < 100 or bounds.bottom - bounds.top < 100:
            return True
        matches.append(int(handle))
        return False

    ctypes.set_last_error(0)
    success = user32.EnumWindows(visit, 0)
    if matches:
        return matches[0]
    if not success:
        error = ctypes.get_last_error()
        if error:
            raise OSError(error, "EnumWindows failed")
    return None


def wait_for_visible_window(
    pid: int,
    poll_exit: Callable[[], int | None],
    *,
    timeout: float = 15.0,
    interval: float = 0.25,
    find_window: Callable[[int], int | None] = find_visible_window,
    platform: str | None = None,
    clock: Callable[[], float] = time.monotonic,
    sleep: Callable[[float], None] = time.sleep,
) -> DesktopWindowObservation:
    """Observe just the launched PID with a bounded, explicitly limited probe."""
    if pid <= 0 or timeout < 0 or interval <= 0:
        raise ValueError("invalid process identifier or readiness timing")
    host = sys.platform if platform is None else platform
    if host != "win32":
        return DesktopWindowObservation("UNSUPPORTED_HOST", pid, "Win32 UI observation required")

    deadline = clock() + timeout
    while True:
        try:
            exit_code = poll_exit()
        except Exception as error:
            return DesktopWindowObservation("PROBE_ERROR", pid, f"process poll failed: {error}")
        if exit_code is not None:
            return DesktopWindowObservation("EARLY_EXIT", pid, "desktop process exited before a visible window", exit_code=exit_code)
        try:
            window = find_window(pid)
        except Exception as error:
            return DesktopWindowObservation("PROBE_ERROR", pid, f"Win32 inspection failed: {error}")
        if window is not None:
            return DesktopWindowObservation("WINDOW_VISIBLE", pid,
                                            "visible native window found; chat and provider NOT certified",
                                            window_handle=window)
        remaining = deadline - clock()
        if remaining <= 0:
            return DesktopWindowObservation("NO_VISIBLE_WINDOW", pid,
                                            "startup timeout without a visible window; process may still be running")
        sleep(min(interval, remaining))
