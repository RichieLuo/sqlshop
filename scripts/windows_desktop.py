#!/usr/bin/env python3
"""Broker installation through the real Explorer desktop without elevation or scheduled tasks."""

import ctypes
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import traceback


def final_path(path):
    """Inspect the actual file handle; package identity cannot detect inherited virtualization."""
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateFileW.argtypes = [ctypes.c_wchar_p, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p,
                                   ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p]
    kernel.CreateFileW.restype = ctypes.c_void_p
    kernel.GetFinalPathNameByHandleW.argtypes = [ctypes.c_void_p, ctypes.c_wchar_p, ctypes.c_uint32, ctypes.c_uint32]
    kernel.GetFinalPathNameByHandleW.restype = ctypes.c_uint32
    kernel.CloseHandle.argtypes = [ctypes.c_void_p]
    handle = kernel.CreateFileW(str(path), 0, 7, None, 3, 0x02000000, None)
    if handle == ctypes.c_void_p(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        buffer = ctypes.create_unicode_buffer(32768)
        length = kernel.GetFinalPathNameByHandleW(handle, buffer, len(buffer), 0)
        if not length or length >= len(buffer):
            raise ctypes.WinError(ctypes.get_last_error())
        return buffer.value
    finally:
        kernel.CloseHandle(handle)


def verify_desktop():
    programs = Path(os.environ["APPDATA"]) / "Microsoft/Windows/Start Menu/Programs"
    actual = final_path(programs)
    expected = os.path.abspath(programs)
    normalized = actual.removeprefix("\\\\?\\")
    private = "\\packages\\" in normalized.lower() and "\\localcache\\" in normalized.lower()
    if private or os.path.normcase(normalized) != os.path.normcase(expected):
        raise OSError(f"Explorer worker still has redirected Start menu access: {actual}")
    return actual


def write_stderr(content):
    if not content:
        return
    if hasattr(sys.stderr, "buffer"):
        sys.stderr.buffer.write(content)
        sys.stderr.buffer.flush()
    else:
        sys.stderr.write(content.decode("utf-8", errors="replace"))


def run_json(arguments, timeout=300):
    """Wait for a verified desktop worker and return its JSON, preserving stderr and exit status."""
    if os.name != "nt":
        raise OSError("The Explorer installation broker requires Windows")
    script_directory = Path(__file__).resolve().parent
    broker = script_directory / "windows" / "launch-desktop.ps1"
    if not broker.is_file():
        broker = script_directory / "launch-desktop.ps1"
    # AppData temp directories share the inherited MSIX view. Profile-root IPC does not.
    exchange = Path(tempfile.mkdtemp(prefix=".shop-install-", dir=Path.home()))
    result_path = exchange / "result.json"
    launched = False
    try:
        request = {"arguments": [str(argument) for argument in arguments], "directory": str(Path.cwd())}
        (exchange / "request.json").write_text(json.dumps(request, ensure_ascii=True), encoding="utf-8")
        windowless_python = Path(sys.executable).with_name("pythonw.exe")
        python = windowless_python if windowless_python.is_file() else Path(sys.executable)
        subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                        "-File", str(broker), "-PythonPath", str(python),
                        "-HelperPath", str(Path(__file__).resolve()), "-ExchangeDirectory", str(exchange)],
                       check=True, creationflags=0x08000000)
        launched = True
        deadline = time.monotonic() + timeout
        while not result_path.is_file():
            if time.monotonic() >= deadline:
                raise OSError(f"Explorer installation worker timed out; diagnostics remain at {exchange}")
            time.sleep(0.05)
        result = json.loads(result_path.read_text(encoding="utf-8"))
        stdout = (exchange / "stdout.bin").read_bytes()
        write_stderr((exchange / "stderr.bin").read_bytes())
        if result["returncode"]:
            raise subprocess.CalledProcessError(result["returncode"], arguments, output=stdout)
        return json.loads(stdout.decode("utf-8-sig"))
    finally:
        if result_path.is_file() or not launched:
            shutil.rmtree(exchange)


def desktop_worker(exchange):
    exchange = Path(exchange)
    result = {"returncode": 1}
    with (exchange / "stdout.bin").open("wb") as stdout, (exchange / "stderr.bin").open("wb") as stderr:
        try:
            result["programs_final_path"] = verify_desktop()
            request = json.loads((exchange / "request.json").read_text(encoding="utf-8"))
            environment = os.environ.copy()
            environment.update(PYTHONIOENCODING="utf-8", PYTHONUTF8="1")
            completed = subprocess.run(request["arguments"], cwd=request["directory"], env=environment,
                                       stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                       creationflags=0x08000000)
            result["returncode"] = completed.returncode
        except Exception:
            stderr.write(traceback.format_exc().encode("utf-8"))
    temporary = exchange / "result.pending"
    temporary.write_text(json.dumps(result, ensure_ascii=True), encoding="utf-8")
    os.replace(temporary, exchange / "result.json")


if __name__ == "__main__":
    if os.name != "nt" or len(sys.argv) != 3 or sys.argv[1] != "--worker":
        raise SystemExit("This helper requires a Windows desktop worker request")
    desktop_worker(sys.argv[2])
