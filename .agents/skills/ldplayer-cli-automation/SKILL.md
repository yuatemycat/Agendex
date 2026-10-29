---
name: ldplayer-cli-automation
description: >-
  Control, automate, and orchestrate LDPlayer 9/4/3 Android emulator instances headlessly and programmatically
  via ldconsole.exe and dnconsole.exe: instance lifecycle (launch, reboot, quit, isrunning), batch fleet management
  (list2, clone, add, modify), direct APK injection (launchex), CPU throttling, and PowerShell/Python automation wrappers.
---

# LDPlayer CLI Console Automation & Fleet Orchestration

This skill provides the authoritative procedures, command specifications, and automation tooling for controlling LDPlayer Android emulator instances natively on Windows hosts via the dedicated CLI console utilities (`ldconsole.exe` for LDPlayer 9, `dnconsole.exe` for LDPlayer 3/4/5).

---

## 1. Architectural Role: Host VM vs In-Guest ADB

Understanding the boundary between host-level VM control and in-guest OS automation is essential:

| Layer | Tooling | Scope of Responsibility |
| :--- | :--- | :--- |
| **Host Hypervisor / VM Layer** | `ldconsole.exe` / `dnconsole.exe` | Cold launch, graceful quit, VM reboot, instance provisioning (`add`/`copy`/`remove`), hardware/resolution configuration, CPU throttling (`downcpu`), window sorting. |
| **In-Guest Android OS Layer** | `adb.exe` (`su 0`) / abstract UNIX sockets | In-guest filesystem, shell commands, process inspection, native library injection (`libhdguest-hook.so`), direct in-memory IPC (`localabstract:sys_render_ipc`). |

> [!IMPORTANT]
> Always use `ldconsole.exe` to start, stop, or reboot instances. Never forcefully terminate LDPlayer processes via `taskkill /f /im dnplayer.exe` or `LdVBoxHeadless.exe` unless a complete lockup occurs, as forceful kills can corrupt the underlying VirtualBox disk images (`.vmdk` / `.nvram`).

---

## 2. Binary Discovery & Multi-Version Topography

LDPlayer CLI executables reside in the root installation folder:

- **LDPlayer 9 (64-bit Android 9 Kernel)**: `ldconsole.exe`
- **LDPlayer 4 / 3 (Android 7 / 5 Kernel)**: `dnconsole.exe`

### Standard Filesystem Candidate Paths
1. `E:\LDPlayer\LDPlayer9\ldconsole.exe` *(Workstation Canonical)*
2. `C:\LDPlayer\LDPlayer9\ldconsole.exe`
3. `D:\LDPlayer\LDPlayer9\ldconsole.exe`
4. `E:\LDPlayer\LDPlayer4.0\dnconsole.exe`
5. `C:\LDPlayer\LDPlayer4.0\dnconsole.exe`
6. `D:\LDPlayer\LDPlayer4.0\dnconsole.exe`
7. `C:\ChangPeng\LDPlayer9\ldconsole.exe` *(Alternate OEM install directory)*

Both binaries accept identical command syntax and parameter switches.

---

## 3. Authoritative Command Specification

All commands follow the pattern:
```powershell
& "<path_to_console.exe>" <command> [parameters]
```

### 3.1 Instance Targeting Conventions
Commands targeting a specific virtual machine accept either:
- `--index <idx>`: Zero-based numeric index (`0` for first instance, `1` for second, etc.)
- `--name "<name>"`: Exact display title string (e.g. `--name "LDPlayer"`)

### 3.2 Core Lifecycle Commands

#### `launch` — Start Instance
```powershell
& "E:\LDPlayer\LDPlayer9\ldconsole.exe" launch --index 0
```

#### `launchex` — Start Instance & Launch APK Immediately
Bypasses the Android desktop launcher and immediately opens the designated package:
```powershell
& "E:\LDPlayer\LDPlayer9\ldconsole.exe" launchex --index 0 --packagename com.supercell.hayday
```

#### `reboot` — Soft-Restart Instance
Reboots the Android guest operating system without terminating the outer host window:
```powershell
& "E:\LDPlayer\LDPlayer9\ldconsole.exe" reboot --index 0
```

#### `quit` — Graceful Shutdown
Cleanly syncs VM disk caches and closes the designated instance:
```powershell
& "E:\LDPlayer\LDPlayer9\ldconsole.exe" quit --index 0
```

#### `quitall` — Shutdown All Running Instances
```powershell
& "E:\LDPlayer\LDPlayer9\ldconsole.exe" quitall
```

---

### 3.3 Instance Telemetry & Introspection

#### `isrunning` — Query Execution State
Returns plain-text `"running"` or `"stop"`:
```powershell
& "E:\LDPlayer\LDPlayer9\ldconsole.exe" isrunning --index 0
```

#### `list2` — Tabular Instance Inventory
Outputs comma-delimited records for all registered instances:
```powershell
& "E:\LDPlayer\LDPlayer9\ldconsole.exe" list2
# Output format:
# Index,Title,TopHandle,BindHandle,AndroidPID,VBoxPID,Unknown,Width,Height,DPI
# Example: 0,LDPlayer,0,0,0,-1,-1,1600,900,240
```
- `VBoxPID`: Process ID of `LdVBoxHeadless.exe` (`-1` if stopped).
- `AndroidPID`: Process ID of emulator main container window (`0` if stopped).

#### `runninglist` — List Only Active Instances
Returns newline-separated titles of running VMs.

---

### 3.4 Hardware, Profile & Fleet Management

#### `modify` — Reconfigure Hardware Specifications
Allows live modification of CPU cores, RAM allocation, resolution, and root privileges:
```powershell
& "E:\LDPlayer\LDPlayer9\ldconsole.exe" modify --index 0 `
    --resolution 1600,900,240 `
    --cpu 4 `
    --memory 4096 `
    --root 1
```

#### `downcpu` — Resource Throttling
Dynamically throttles host CPU consumption of an instance (useful for background headless scaling):
```powershell
& "E:\LDPlayer\LDPlayer9\ldconsole.exe" downcpu --index 0 --rate 50
```

#### `add` / `copy` / `remove` — Fleet Provisioning
- **Create fresh VM**: `& ldconsole.exe add --name "Farm-Worker-01"`
- **Clone existing VM**: `& ldconsole.exe copy --name "Farm-Worker-02" --from 0`
- **Delete VM**: `& ldconsole.exe remove --index 2`

#### `runapp` / `killapp` — Direct App Control
- **Start app**: `& ldconsole.exe runapp --index 0 --packagename com.supercell.hayday`
- **Force stop app**: `& ldconsole.exe killapp --index 0 --packagename com.supercell.hayday`

---

## 4. Workstation Invariants & Port Mapping

LDPlayer instances bind deterministic ADB TCP host ports and internal device identifiers:

| Instance Index | Display Name | ADB Device ID | Host ADB Port | VBox Console Port |
| :--- | :--- | :--- | :--- | :--- |
| `0` | `LDPlayer` | `emulator-5554` | `127.0.0.1:5555` | `127.0.0.1:5554` |
| `1` | `LDPlayer-1` | `emulator-5556` | `127.0.0.1:5557` | `127.0.0.1:5556` |
| `2` | `LDPlayer-2` | `emulator-5558` | `127.0.0.1:5559` | `127.0.0.1:5558` |
| `N` | `LDPlayer-N` | `emulator-(5554+2N)` | `127.0.0.1:(5555+2N)` | `127.0.0.1:(5554+2N)` |

### Workstation Safety Rules
1. **Root Elevation Invariant**: LDPlayer 9 implements a custom root daemon. **NEVER** run `adb root` (which kills `adbd` and locks transport); always use `su 0` directly in shell invocations:
   ```bash
   adb -s emulator-5554 shell su 0 "<command>"
   ```
2. **Coexistence with QEMU**: When running AOSP/QEMU alongside LDPlayer, start QEMU with `-port 5564` to prevent port `5554` collisions.
3. **Cold Boot Tolerance**: Allow 15–25 seconds after `ldconsole launch` before expecting ADB connectivity. Verify boot completion via:
   ```powershell
   adb -s emulator-5554 shell getprop sys.boot_completed
   ```

---

## 5. Automation Tooling Reference

Two production wrappers are maintained in this skill's `scripts/` directory:

### 5.1 PowerShell Utility: `ldplayer_control.ps1`
Location: [`scripts/ldplayer_control.ps1`](file:///E:/02_Projects/Intra-farm/.agents/skills/ldplayer-cli-automation/scripts/ldplayer_control.ps1)

```powershell
# List all instances in a formatted table
powershell -ExecutionPolicy Bypass -File "E:\02_Projects\Intra-farm\.agents\skills\ldplayer-cli-automation\scripts\ldplayer_control.ps1" -Action List

# Launch instance 0 directly into Hay Day
powershell -ExecutionPolicy Bypass -File "E:\02_Projects\Intra-farm\.agents\skills\ldplayer-cli-automation\scripts\ldplayer_control.ps1" -Action Launch -Index 0 -PackageName com.supercell.hayday

# Query status
powershell -ExecutionPolicy Bypass -File "E:\02_Projects\Intra-farm\.agents\skills\ldplayer-cli-automation\scripts\ldplayer_control.ps1" -Action Status -Index 0

# Graceful quit
powershell -ExecutionPolicy Bypass -File "E:\02_Projects\Intra-farm\.agents\skills\ldplayer-cli-automation\scripts\ldplayer_control.ps1" -Action Quit -Index 0
```

### 5.2 Python Controller: `ldplayer_controller.py`
Location: [`scripts/ldplayer_controller.py`](file:///E:/02_Projects/Intra-farm/.agents/skills/ldplayer-cli-automation/scripts/ldplayer_controller.py)

```python
from scripts.ldplayer_controller import LdPlayerController

controller = LdPlayerController()

# Check and launch if stopped
if not controller.is_running(0):
    controller.launch(0, package_name="com.supercell.hayday")
    controller.wait_for_running(0, timeout=45.0)

# Inspect registered instances
for inst in controller.list_instances():
    print(f"VM #{inst.index}: {inst.name} -> Running: {inst.is_running}")
```
