---
name: optimumu-runtime-engineering
description: >-
  Procedures for building, configuring, and operating Optimumu: a dedicated, lightweight,
  hardware-accelerated Android micro-emulator (Google QEMU x86_64) tailored for Hay Day,
  bypassing Promon SHIELD anti-tamper crashes via ARM64 libndk_translation patching,
  DEX exception neutralization, environment sanitation, and zero-disk in-memory IPC bridging.
---

# Optimumu Dedicated Micro-Emulator & Anti-Tamper Runtime Engineering

Optimumu is a dedicated, high-performance Android runtime designed to execute Supercell's Hay Day headlessly or with GPU acceleration without reliance on commercial third-party emulators (MEmu, LDPlayer, Nox). It eliminates Promon SHIELD hypervisor detection exits, resolves Intel Houdini instruction crashes, and directly bridges in-guest game automation to Windows tooling.

---

## 1. Architectural Overview

```text
+---------------------------------------------------------------------------------------+
| WINDOWS HOST (Direct3D 12 / Vulkan on Dedicated GPU)                                   |
| Port :48899 <-------- ADB Port Forward (tcp:48899 localabstract:sys_render_ipc) ------+
| Controller: tools/emulator/optimumu_manager.py                                         |
+---------------------------------------------------------------------------------------+
                                        | (WHPX Hypervisor)
                                        v
+---------------------------------------------------------------------------------------+
| OPTIMUMU GUEST (Google QEMU Core / Android 11 x86_64 / Pixel 7 Profile)               |
|                                                                                       |
| 1. System Image: system-images;android-30;google_apis_playstore;x86_64                |
|    - Ships Google libndk_translation (x86_64, x86, arm64-v8a) out-of-the-box         |
|    - Certified Google Play production build (zero -writable-system / adb root req)    |
| 2. Game Process: com.supercell.hayday (arm64-v8a native libsupercell_hayday.so)       |
| 3. In-Guest IPC: \0sys_render_ipc (abstract UNIX socket forwarded to :48899)          |
+---------------------------------------------------------------------------------------+
```

---

## 2. AVD Configuration & QEMU CLI Invariants

- **Tool Path**: `E:\03_Devops\AndroidSdk\emulator\emulator.exe`
- **AVD Profile**: Pixel 7 (`--device pixel_7`)
- **Key INI Settings** (`~/.android/avd/Optimumu_HayDay.avd/config.ini`):
  ```ini
  hw.cpu.ncore = 4
  hw.ramSize = 4096
  vm.heapSize = 512
  disk.dataPartition.size = 8G
  hw.gpu.enabled = yes
  hw.gpu.mode = host
  hw.dPad = no
  hw.mainKeys = no
  hw.trackBall = no
  fastboot.forceColdBoot = yes
  fastboot.forceFastBoot = no
  fastboot.forceChosenSnapshotBoot = no
  firstboot.saveToLocalSnapshot = no
  firstboot.bootFromLocalSnapshot = no
  firstboot.bootFromDownloadableSnapshot = no
  saveOnExit = false
  ```
- **Mandatory CLI Flags & Invariants**:
  - **Singular Port**: Always pass `-port 5554` (singular). Never use `-ports 5554,5555` which causes QEMU to bind dynamic IPv6 ports.
  - **OpenGL Environment**: Set `QT_OPENGL=desktop` in the execution environment to avoid missing `opengl32sw.dll` software fallbacks.
  - **Crash & Snapshot Guards**: Always pass `-accel on -no-snapshot -no-boot-anim -no-audio -crash-report-mode disabled -timezone Etc/UTC`.
  - **Headless Mode**: Use `-no-window -gpu swiftshader_indirect` for reliable, crash-free headless background operation.
  - **GUI Mode**: Use `-gpu host` when a visible emulator window is required.
  - **Detached Process Lifecycle (Windows)**: When spawning background emulator instances in Python, use `creationflags = subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.DETACHED_PROCESS` with `stdin=subprocess.DEVNULL`, `close_fds=True`, and immediately close parent pipe/file handles (`out_log.close()`, `err_log.close()`) so the child retains exclusive ownership and the parent CLI returns cleanly without handle inheritance hangs.
  - **Clean Cold-Boot State**: Before launching, auto-wipe `snapshots/`, `*.lock`, and `hardware-qemu.ini` from the AVD directory to eliminate "bad color buffer handle" errors on fresh boots.
  - **Persistent ADB Server Lifecycle**: Start a single ADB daemon on port 5037 before launching; never restart or kill ADB mid-run. Treat initial 15-20s `offline` status as normal kernel boot; only nudge `adb reconnect offline` on stalls (>25s offline).

---

## 3. Google `libndk_translation` Provisioning & Binary Patches

### 3.1 Provisioning the ARM64 Translation Suite
Google APIs images default to `x86_64` only. To support `arm64-v8a`:
1. Extract `E:\03_Devops\libndk_translation.tar` into guest `/`:
   ```bash
   adb -s emulator-5554 push "E:\03_Devops\libndk_translation.tar" /data/local/tmp/
   adb -s emulator-5554 shell "cd / && tar -xf /data/local/tmp/libndk_translation.tar"
   ```
2. Label `app_process64` with `u:object_r:zygote_exec:s0` to prevent Zygote post-fork aborts:
   ```bash
   adb -s emulator-5554 shell "chcon u:object_r:zygote_exec:s0 /system/bin/arm64/app_process64; chmod 755 /system/bin/arm64/app_process64 /system/bin/arm64/linker64"
   ```
3. Update `/system/build.prop` and `/vendor/build.prop` with:
   ```properties
   ro.product.cpu.abilist=x86_64,arm64-v8a
   ro.product.cpu.abilist64=x86_64,arm64-v8a
   ro.dalvik.vm.native.bridge=libndk_translation.so
   ro.enable.native.bridge.exec=1
   ro.ndk_translation.version=0.2.3
   ```

### 3.2 4-Part `libndk_translation.so` Patchset (`patch_ndk_stable.py`)
Google's official `libndk_translation` crashes on Promon Shield anti-tamper checks without four binary patches:
1. **Custom `sa_restorer` Signal Trampoline Bypass**:
   - File offset `0x1990b0`: `eb 0c`
   - File offset `0x1990b8`: `90 90 90 90 90 90` (6x NOPs)
   - File offset `0x1991dc`: `eb 28`
   - File offset `0x199200`: `90 90 90 90 90 90` (6x NOPs)
2. **ELF `PT_LOAD` 1 RW Memory Mapping Patch**:
   - File offset `0x7c`: Change `p_flags` from `0x04` (`PF_R`) to `0x06` (`PF_R | PF_W`) to eliminate `SEGV_ACCERR` on `argv[0]` scrubbing.
3. **`LightTranslator::UndefinedInsn` Neutralization (JIT `brk #1` Trap Bypass)**:
   - File offset `0x195680`: Change first byte to `0xc3` (`ret`).
   - Prevents JIT emission of the `call UndefinedInsn` + infinite comparison loop, allowing Promon's `brk #1` trap stubs to fall through to `ret` cleanly.
4. **`HeavyOptimizerFrontend::UndefinedInsn` Neutralization**:
   - File offset `0xd0e90`: Change first byte to `0xc3` (`ret`).
5. **Runtime C++ Function Integrity**:
   - Keep file offset `0x20f720` (`_ZN15ndk_translation13UndefinedInsnEm`) 100% untouched to prevent C++ stack frame corruption and Zygote bootloops.
6. **Offset Portability & the `0x110538` Trap (observed 2026-10-10)**:
   - The `0xde718` NOP target is build-specific. On the ARM guest build (`scratch/guest_libndk.so`, 1,272,208 B) the `0f 85 94 00 00 00` jcc lives at `0xde718`; on the x86_64 api30 build (`scratch/api30_libndk.so`, 1,654,288 B) those same bytes sit at `0x110538` (`WrapGuestFunctionImpl` — never patch; modifying it breaks guest TLS and causes `SEGV_MAPERR`), while `0xde718` holds unrelated bytes (`d0 0d 66 0f 3a 14`).
   - `libndk_translation_api30_patched.so` shipped NOP-patched at `0x110538` — the invariant violated. Result: the JIT trap spin — infinite `mmap(NULL,131072)`/`munmap` + `gettid()` + `rt_sigprocmask [QUIT USR1 PIPE RTMIN]` loop in `Optimumu/trace_all.log` (lines 34767→EOF), no first OpenGL frame.
   - **Fixed artifact**: `Optimumu/libndk_translation_api30_fixed.so` — byte-identical except `0x110538` restored to `0f 85 94 00 00 00`. Deploy this instead of `..._api30_patched.so`.
   - **Cold-Boot Verification Gate**: after deploying any `libndk_translation` variant, the strace tail must NOT show the 131072 mmap-spin loop, and `dumpsys SurfaceFlinger --list | grep -i supercell` must show a live layer, not `Splash Screen`.

---

## 4. Promon SHIELD Java Exception Neutralization

Promon executes a background watchdog thread (`yuke.R`) that invokes `yuke.C.a` on detection, triggering an uncaught exception (`yuke.j`).

### In-Place DEX Neutralization:
1. In `classes2.dex`:
   - Patch `yuke.C.a:(Ljava/lang/String;)V` at offset `0x82e8ec` -> `0e00` (`return-void`).
   - Patch `yuke.R.run:()V` at offset `0x82f038` -> `0e00` (`return-void`).
2. Recompute SHA-1 (bytes 12..31) and Adler32 (bytes 8..11).
3. Splice patched DEX back into `base.apk` without full re-alignment:
   - Calculate CRC32 of patched DEX.
   - Update CRC32 in local zip header (`offset 14..18`) and central directory header.

---

## 5. System Environment Sanitation & Root Masking

Promon inspects standard userdebug and root markers on startup:
1. **Hide `su` Binary**:
   ```bash
   adb shell "mv /system/xbin/su /system/xbin/su_hidden"
   ```
2. **Sanitize `build.prop`**:
   ```bash
   adb shell "sed -i 's/ro.build.tags=dev-keys/ro.build.tags=release-keys/g' /system/build.prop"
   adb shell "sed -i 's/ro.build.type=userdebug/ro.build.type=user/g' /system/build.prop"
   ```
3. **SELinux State**:
   Optimumu defaults to `Enforcing` mode, preventing Promon's SELinux disablement triggers.

---

## 6. Abstract Socket IPC Bridging

To connect Windows GUI suites ([`tools/gui/intra_farm_qt_v2.py`](file:///E:/02_Projects/Intra-farm/tools/gui/intra_farm_qt_v2.py)) directly to the in-guest companion hook:
```powershell
& "E:\03_Devops\AndroidSdk\platform-tools\adb.exe" -s emulator-5554 forward tcp:48899 localabstract:sys_render_ipc
```
This enables zero-disk in-memory command dispatches (`GameMode::addCommand`) at sub-millisecond latencies.

---

## 7. Master Controller CLI Contract

The entire Optimumu lifecycle is encapsulated in [`tools/emulator/optimumu_manager.py`](file:///E:/02_Projects/Intra-farm/tools/emulator/optimumu_manager.py):
- **Start Emulator**:
  - `python tools/emulator/optimumu_manager.py start` (detached background daemon, returns immediately with exit code 0)
  - `python tools/emulator/optimumu_manager.py start --wait` (blocks until `sys.boot_completed == 1`, configures IPC/ports, exits 0 on success, 1 on timeout)
- **Query Status**:
  - `python tools/emulator/optimumu_manager.py status` (human-readable table; exit 0 if online, 1 if stopped/offline)
  - `python tools/emulator/optimumu_manager.py status --json` (pure machine-readable JSON to stdout, logs to stderr; exit 0 if online, 1 if stopped)
- **Install Hay Day (Atomic Multi-Split)**:
  - `python tools/emulator/optimumu_manager.py install-hayday` (fail-fast ABI check verifying `arm64-v8a` before installation; installs 4 APKM splits + asset pack via `install-multiple`; exits 0 on success, 1 on failure)
- **Check ABI Properties**:
  - `python tools/emulator/optimumu_manager.py check-abi [--json]` (inspects `ro.product.cpu.abilist`, `ro.dalvik.vm.native.bridge`, `ro.build.type`, `ro.build.tags`; exits 0 if `arm64-v8a` supported, 1 if not)
- **Launch Hay Day**:
  - `python tools/emulator/optimumu_manager.py launch` (force-stops existing instance and launches via monkey)
- **Bridge Abstract IPC**:
  - `python tools/emulator/optimumu_manager.py bridge` (configures `:48899 -> localabstract:sys_render_ipc` and `:21503 -> :5555`)
- **Capture Screenshot**:
  - `python tools/emulator/optimumu_manager.py screenshot --out screen.png`
- **Dump Logcat**:
  - `python tools/emulator/optimumu_manager.py logcat --out logcat.txt`
- **Stop Emulator**:
  - `python tools/emulator/optimumu_manager.py stop` (graceful shutdown via `emu kill` with process-level fallback)

### Chained Unattended Invocation:
```powershell
python tools/emulator/optimumu_manager.py start --wait && python tools/emulator/optimumu_manager.py install-hayday
```
Chains purely on deterministic exit code 0 without polling loops or manual intervention.
