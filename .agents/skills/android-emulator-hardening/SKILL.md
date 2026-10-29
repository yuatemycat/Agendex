---
name: android-emulator-hardening
description: >-
  Procedures and invariants for running headless AOSP/QEMU Android emulators on Windows 11
  without flakiness: persistent ADB server lifecycle management, SwiftShader indirect GPU execution,
  detached child process survival without pipe hangs, clean cold-boot cache hygiene against color-buffer
  corruption, fail-fast ABI validation, and deterministic exit-code CLI contracts.
---

# Android Emulator Lifecycle & Headless Hardening

This skill defines the authoritative infrastructure invariants for running Google AOSP/QEMU Android emulators (via `emulator.exe`) on Windows 11 hosts, specifically preventing common failure modes: ADB "device offline" races, pipe handle inheritance hangs, bad color buffer handle crashes, and silent fallback to software CPU emulation.

---

## 1. Persistent ADB Lifecycle & Network Invariants

### 1.1 Single Persistent Daemon
- **Rule**: Start exactly one persistent ADB server on `127.0.0.1:5037` before launching the emulator.
- **Never Kill Mid-Run**: Never invoke `adb kill-server` during automation or retry loops. Killing the server resets transport state for all active emulators and drops authenticated USB/TCP tunnels.
- **Singular Port Argument**: Always specify `-port 5554` (singular).
  - *Anti-Pattern*: Specifying `-ports 5554,5555` causes QEMU to ignore the argument and allocate dynamic, unpredictable IPv6/IPv4 ports.
- **Purge Relay Workarounds**: Never use `bridge_ip.py`, `::1` TCP relays, or `adb connect 127.0.0.1:5555` loops for local emulators. Google QEMU connects directly to the local ADB server on port 5037 when started with `-port 5554`.

### 1.2 Boot Polling & Offline Stall Tolerance
During the initial 15–25 seconds of cold boot, `adbd` naturally reports `offline` while the Android kernel initializes and crypto keys are exchanged:
- **Do Not Panic on `offline`**: Do not treat early `offline` state as a boot failure.
- **Polling Loop**: Continuously poll `getprop sys.boot_completed` with a 90–120s timeout.
- **Nudge on True Stall**: Only issue `adb reconnect offline` if the device has been in `offline` state continuously for more than 25 seconds and at least 20 seconds have elapsed since the last reconnect attempt.

```python
def wait_for_boot(device: str = "emulator-5554", timeout: int = 120, adb_bin: str = "adb") -> bool:
    start = time.time()
    last_reconnect = start
    while time.time() - start < timeout:
        try:
            res = subprocess.run([adb_bin, "-s", device, "shell", "getprop", "sys.boot_completed"],
                                 capture_output=True, text=True, timeout=5)
            if res.returncode == 0 and res.stdout.strip() == "1":
                return True
            # Normal boot: device is offline for first ~20s. On stall (>25s offline), nudge reconnect.
            if (time.time() - start > 25) and (time.time() - last_reconnect > 20):
                st = subprocess.run([adb_bin, "-s", device, "get-state"], capture_output=True, text=True, timeout=3)
                if "offline" in st.stdout:
                    subprocess.run([adb_bin, "reconnect", "offline"], capture_output=True, text=True, timeout=5)
                    last_reconnect = time.time()
        except Exception:
            pass
        time.sleep(2)
    return False
```

### 1.3 Windows 11 Job Object Process Isolation & Post-Start Handshake Settling
- **Windows Job Object Hazard**: Child processes spawned within non-detached PowerShell sessions or ephemeral CLI runners are automatically terminated when the host session closes. Always start persistent ADB daemons with detached process creation flags (`DETACHED_PROCESS` / `CREATE_NEW_PROCESS_GROUP`).
- **Post-Start Handshake Settling**: During the initial 1.0–2.0 seconds after `adb start-server`, the adb transport establishes TCP sockets and authenticates with emulator instances (e.g. MEmu `:21503` or LDPlayer `:5555`). Queries issued immediately within <500ms of daemon start often return `device offline` or `device not found`. Automation scripts must include a 1.0–2.0s settling delay after server initialization before issuing device-specific commands.

---

## 2. Headless Launch & Detached Process Survival

### 2.1 Headless CLI Flags
To run completely headless without GUI popups, audio server allocations, or crash report dialogs:
```powershell
& "<sdk_path>\emulator\emulator.exe" `
    -avd <avd_name> `
    -port 5554 `
    -accel on `
    -no-snapshot `
    -no-boot-anim `
    -no-audio `
    -no-window `
    -gpu swiftshader_indirect `
    -crash-report-mode disabled `
    -timezone Etc/UTC
```
- `-gpu swiftshader_indirect`: Prevents host GPU driver crashes and headless DXGI context creation failures on Windows.
- `-accel on`: Ensures hardware hypervisor acceleration (WHPX) is strictly required; fails loudly instead of silently dropping into 10x slower software CPU emulation.
- `-crash-report-mode disabled`: Suppresses modal crash dialogs and `%TEMP%\AndroidEmulator` dump accumulation.

### 2.2 Windows Detached Process Survival (Preventing Pipe Deadlocks)
When launching an emulator subprocess from Python in Windows that must persist after the Python script exits:
1. Pass `creationflags = subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.DETACHED_PROCESS`.
2. Pass `stdin=subprocess.DEVNULL`.
3. Set `close_fds=True`. On Windows Python 3.7+, `close_fds=True` utilizes `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` so child processes do not inherit caller stdout/stderr pipe handles.
4. Explicitly close the file descriptors (`out_log.close()`, `err_log.close()`) in the parent process immediately after `Popen`.
   *Critical Invariant*: If parent handles are left open, callers using `subprocess.run(capture_output=True)` to launch the manager CLI will hang indefinitely waiting for EOF on inherited pipes.

```python
creationflags = subprocess.CREATE_NEW_PROCESS_GROUP
if os.name == "nt":
    creationflags |= getattr(subprocess, "DETACHED_PROCESS", 0x00000008)

proc = subprocess.Popen(
    cmd,
    stdin=subprocess.DEVNULL,
    stdout=out_log,
    stderr=err_log,
    creationflags=creationflags,
    close_fds=True,
    env=env
)
out_log.close()
err_log.close()
```

---

## 3. Clean Cold-Boot State & Buffer Stability

### 3.1 The "Bad Color Buffer Handle" Problem
Stale QEMU snapshot state and lockfiles frequently corrupt OpenGL buffer allocations across restarts, manifesting as:
```text
E/emuglGLESv2_enc: a_rc_e: 0x... rcReadColorBuffer: bad color buffer handle
```

### 3.2 Automated Sanitization Routine
Before launching any cold-boot session, sanitize the AVD directory (`~/.android/avd/<avd_name>.avd/`):
1. **Wipe `snapshots/` Directory**: Recursively delete `~/.android/avd/<avd_name>.avd/snapshots/`.
2. **Remove Lockfiles**: Delete any stale `*.lock` files (`hardware-qemu.ini.lock`, `multiinstance.lock`).
3. **Delete `hardware-qemu.ini`**: Remove cached hardware runtime state so QEMU regenerates parameters cleanly from `config.ini`.
4. **Enforce Cold Boot Invariants in `config.ini`**:
   ```ini
   fastboot.forceColdBoot = yes
   fastboot.forceFastBoot = no
   fastboot.forceChosenSnapshotBoot = no
   firstboot.saveToLocalSnapshot = no
   firstboot.bootFromLocalSnapshot = no
   firstboot.bootFromDownloadableSnapshot = no
   saveOnExit = false
   ```

---

## 4. Fail-Fast Architecture & Base Image Selection

### 4.1 Native ARM Translation (Google Play Images)
- Use `system-images;android-30;google_apis_playstore;x86_64` (or certified Play Store images that natively ship Google's `libndk_translation`).
- This image provides native support for `x86_64, x86, arm64-v8a, armeabi-v7a, armeabi` out-of-the-box without requiring `-writable-system`, root modification, or manual tar extraction.

### 4.2 Fail-Fast ABI Pre-Flight Check
Before extracting or installing 64-bit ARM application splits, query the runtime ABI list:
```python
res = subprocess.run([adb, "-s", device, "shell", "getprop", "ro.product.cpu.abilist"],
                     capture_output=True, text=True)
abilist = [a.strip() for a in res.stdout.strip().split(",") if a.strip()]
if "arm64-v8a" not in abilist:
    print(f"[-] Diagnosis: Device {device} lacks required ABI 'arm64-v8a' (supported: {','.join(abilist)}); system image does not have ARM translation enabled.")
    sys.exit(1)
```
This fails in <100ms with a clean one-line diagnosis instead of throwing `INSTALL_FAILED_NO_MATCHING_ABIS` deep in multi-split installation flows.

---

## 5. Deterministic CLI Contract & Exit Codes

Every automation command must adhere to a strict exit code contract: **Exit code 0 ONLY on verified success; non-zero (1) on any failure or stopped state.**

### 5.1 Subcommands
| Command | Success (0) | Failure (1) | Notes |
| :--- | :--- | :--- | :--- |
| `start` | Background process spawned | Process failed to launch | Returns immediately |
| `start --wait` | Device booted (`sys.boot_completed=1`) & IPC ready | Boot timed out or crashed | Idempotent: returns 0 if already online |
| `status` | Device online and fully booted | Device stopped, booting, or offline | Emits human table or JSON |
| `install-hayday` | All splits installed & package verified | ABI mismatch or install failed | Fail-fast diagnosis on ABI missing |
| `check-abi` | `arm64-v8a` present in abilist | `arm64-v8a` missing | Fast pre-flight verification |
| `stop` | Device shutdown confirmed | Shutdown timed out | Fallback to PID kill |

### 5.2 Machine-Readable `--json` Protocol
When `--json` is supplied:
- All human logging, diagnostics, and informational banners (`[+] ...`, `[-] ...`) are routed to `sys.stderr`.
- Only valid, machine-readable JSON is emitted to `sys.stdout`.
- Exit codes still reflect operation success/failure.

### 5.3 Pure Chaining Without Polling
Because `start --wait` exits 0 only when the emulator is fully online and ready, multi-step orchestration chains purely via standard shell exit codes:
```powershell
python tools/emulator/optimumu_manager.py start --wait && python tools/emulator/optimumu_manager.py install-hayday
```
Eliminates all custom polling loops, sleep timers, and flakiness in CI/CD pipelines and automated batch runners.
