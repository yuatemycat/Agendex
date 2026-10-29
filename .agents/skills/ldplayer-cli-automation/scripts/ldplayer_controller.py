"""
Authoritative Python LDPlayer 9 / 4 CLI Controller.
Provides typed, programmatic control over LDPlayer instances via ldconsole.exe / dnconsole.exe.
"""

from __future__ import annotations

import os
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional, Union


DEFAULT_CANDIDATE_PATHS = [
    Path(r"E:\LDPlayer\LDPlayer9\ldconsole.exe"),
    Path(r"C:\LDPlayer\LDPlayer9\ldconsole.exe"),
    Path(r"D:\LDPlayer\LDPlayer9\ldconsole.exe"),
    Path(r"E:\LDPlayer\LDPlayer4.0\dnconsole.exe"),
    Path(r"C:\LDPlayer\LDPlayer4.0\dnconsole.exe"),
    Path(r"D:\LDPlayer\LDPlayer4.0\dnconsole.exe"),
    Path(r"C:\ChangPeng\LDPlayer9\ldconsole.exe"),
    Path(r"D:\ChangPeng\LDPlayer9\ldconsole.exe"),
    Path(r"E:\ChangPeng\LDPlayer9\ldconsole.exe"),
]


@dataclass
class LdInstance:
    index: int
    name: str
    top_hwnd: int
    bind_hwnd: int
    android_pid: int
    vbox_pid: int
    width: int
    height: int
    dpi: int

    @property
    def is_running(self) -> bool:
        return self.vbox_pid > 0 or self.android_pid > 0

    @property
    def adb_device_id(self) -> str:
        # LDPlayer index 0 binds to port 5555 (emulator-5554)
        # Index n binds to port 5555 + n*2
        base_port = 5554 + (self.index * 2)
        return f"emulator-{base_port}"


class LdPlayerController:
    """Programmatic wrapper around ldconsole.exe / dnconsole.exe."""

    def __init__(self, console_path: Optional[Union[str, Path]] = None):
        if console_path:
            p = Path(console_path)
            if not p.exists():
                raise FileNotFoundError(f"Specified LDPlayer console does not exist: {p}")
            self.console_exe = p
        else:
            self.console_exe = self.find_console()

    @staticmethod
    def find_console() -> Path:
        for candidate in DEFAULT_CANDIDATE_PATHS:
            if candidate.exists():
                return candidate
        raise FileNotFoundError(
            "LDPlayer console binary (ldconsole.exe / dnconsole.exe) was not found in standard paths."
        )

    def _exec(self, *args: str, timeout: float = 30.0) -> subprocess.CompletedProcess[str]:
        cmd = [str(self.console_exe), *args]
        return subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )

    @staticmethod
    def _target_args(target: Union[int, str]) -> List[str]:
        if isinstance(target, int) or (isinstance(target, str) and target.isdigit()):
            return ["--index", str(target)]
        return ["--name", str(target)]

    def list_instances(self) -> List[LdInstance]:
        """Lists all registered VM instances with resolution and running state."""
        res = self._exec("list2")
        instances: List[LdInstance] = []
        for line in res.stdout.strip().splitlines():
            parts = [p.strip() for p in line.split(",") if p.strip()]
            if len(parts) >= 6:
                try:
                    idx = int(parts[0])
                    name = parts[1]
                    top_h = int(parts[2]) if parts[2].lstrip("-").isdigit() else 0
                    bind_h = int(parts[3]) if parts[3].lstrip("-").isdigit() else 0
                    a_pid = int(parts[4]) if parts[4].lstrip("-").isdigit() else -1
                    v_pid = int(parts[5]) if parts[5].lstrip("-").isdigit() else -1
                    w = int(parts[7]) if len(parts) > 7 and parts[7].isdigit() else 0
                    h = int(parts[8]) if len(parts) > 8 and parts[8].isdigit() else 0
                    dpi = int(parts[9]) if len(parts) > 9 and parts[9].isdigit() else 0

                    instances.append(
                        LdInstance(
                            index=idx,
                            name=name,
                            top_hwnd=top_h,
                            bind_hwnd=bind_h,
                            android_pid=a_pid,
                            vbox_pid=v_pid,
                            width=w,
                            height=h,
                            dpi=dpi,
                        )
                    )
                except ValueError:
                    continue
        return instances

    def is_running(self, target: Union[int, str] = 0) -> bool:
        """Returns True if the target instance is actively running."""
        res = self._exec("isrunning", *self._target_args(target))
        return "running" in res.stdout.lower()

    def launch(self, target: Union[int, str] = 0, package_name: Optional[str] = None) -> bool:
        """Launches an instance, optionally directly launching an APK."""
        args = ["launchex", *self._target_args(target), "--packagename", package_name] if package_name else ["launch", *self._target_args(target)]
        res = self._exec(*args)
        return res.returncode == 0

    def reboot(self, target: Union[int, str] = 0) -> bool:
        """Reboots an instance."""
        res = self._exec("reboot", *self._target_args(target))
        return res.returncode == 0

    def quit(self, target: Union[int, str] = 0) -> bool:
        """Gracefully quits an instance."""
        res = self._exec("quit", *self._target_args(target))
        return res.returncode == 0

    def quit_all(self) -> bool:
        """Gracefully quits all instances."""
        res = self._exec("quitall")
        return res.returncode == 0

    def run_app(self, target: Union[int, str], package_name: str) -> bool:
        """Launches an application inside the target VM."""
        res = self._exec("runapp", *self._target_args(target), "--packagename", package_name)
        return res.returncode == 0

    def kill_app(self, target: Union[int, str], package_name: str) -> bool:
        """Force-kills an application inside the target VM."""
        res = self._exec("killapp", *self._target_args(target), "--packagename", package_name)
        return res.returncode == 0

    def wait_for_running(self, target: Union[int, str] = 0, timeout: float = 45.0, poll_interval: float = 1.0) -> bool:
        """Blocks until the target VM enters 'running' state or timeout expires."""
        start = time.time()
        while time.time() - start < timeout:
            if self.is_running(target):
                return True
            time.sleep(poll_interval)
        return False


if __name__ == "__main__":
    controller = LdPlayerController()
    print(f"[+] Found LDPlayer Console: {controller.console_exe}")
    instances = controller.list_instances()
    print(f"[+] Total Instances: {len(instances)}")
    for inst in instances:
        status_str = "RUNNING" if inst.is_running else "STOPPED"
        print(f"    Index {inst.index}: '{inst.name}' -> {status_str} ({inst.width}x{inst.height}@{inst.dpi}dpi)")
