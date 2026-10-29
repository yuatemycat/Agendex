---
name: android-emulator-fleet-safety
description: >-
  Authoritative Android emulator mapping and safety rules on this workstation. Enforces
  device identification before running adb commands to protect active game instances
  and isolate test sandboxes across Intra-farm, ODM-Native, and Meteor projects.
---

# Android Emulator Fleet & Safety Rules

Before executing any `adb`, `memuc`, or shell-automation command, the agent MUST identify the connected target device serial. Multiple emulators run simultaneously on this host for completely different purposes.

## 1. Device Fleet Routing Matrix

| Serial / Port | Instance / Name | Project / Purpose | Permissions & Restrictions |
| :--- | :--- | :--- | :--- |
| **`emulator-5564`** | `Optimumu_HayDay` (AVD) | **Live Farm Automation** | ⛔ **STRICTLY OFF LIMITS**. User's live game process. NEVER install APKs, stop processes, reboot, run monkey tests, or mutate root. Read-only adb probes only with explicit user permission. |
| **`emulator-5580`** | `Resurrection_API30` (AVD) | **Meteor / Sandbox** | Headless root target for OS sandbox and container experimentation. Stored at `E:\03_Devops\AndroidSdk\avd-resurrection`. Booted with `-port 5580`. `adb root` is supported. |
| **`127.0.0.1:21513`** | `MEmu_1` (MEmu VM) | **ODM-Native** | Authorized target for ODM Downloader test runs (`com.woaiyj1314.odm`). Launch with `Start-Process MEmu.exe MEmu_1` from `E:\01_Apps\Program Files\Microvirt\MEmu`. Always pass `-s 127.0.0.1:21513`. |
| **`127.0.0.1:21503`** | `MEmu_0` (Index 0) | System / Default | ⚠️ **DO NOT TOUCH**. Leave index 0 unmodified. |
| **`emulator-5556`** | `SH_M24` / Port 5557 | Unidentified | ⚠️ Refused console queries; leave alone. |

## 2. Pre-Command Identification Protocol

Never run blind commands like `adb install` or `adb shell` without `-s <serial>`.
```powershell
# For Android Virtual Devices (AVD):
adb -s <serial> emu avd name

# For MEmu instances:
& "E:\01_Apps\Program Files\Microvirt\MEmu\memuc.exe" listvms
```
