---
name: windows-devops-toolchains
description: >-
  Activate and execute development toolchains on this Windows machine using the portable
  E:\03_Devops installation. Covers Rust/Cargo (MSVC env.bat), Android SDK, JDK 21, Python 3.12,
  Flutter, Node.js/npx, Rclone, and ffmpeg, plus PowerShell 5.1 git commit quoting safeguards.
---

# Windows DevOps Toolchains & Environment Runbook

All major development toolchains on this workstation are centralized under `E:\03_Devops\`. Standard terminal sessions often do not have these tools on the default system `PATH`, and paths under `D:\` (e.g. `D:\Android\Sdk`, `D:\DevTools`) are obsolete.

## 1. Toolchain Locations Quick Reference

| Toolchain | Path | Notes / Invocation Pattern |
| :--- | :--- | :--- |
| **MSVC / Rust / C++** | `E:\03_Devops\env.bat` | `cmd /c "call E:\03_Devops\env.bat >nul && <command>"` |
| **Android SDK** | `E:\03_Devops\AndroidSdk` | Set `$env:ANDROID_HOME = 'E:\03_Devops\AndroidSdk'` |
| **JDK 21** | `E:\03_Devops\JDK` | Set `$env:JAVA_HOME = 'E:\03_Devops\JDK'` |
| **Platform Tools (adb)** | `E:\03_Devops\platform-tools\adb.exe` | Add to PATH or call by absolute path |
| **Python 3.12** | `E:\03_Devops\Python\python.exe` | Run with `-I` to prevent user site contamination |
| **Node.js / npx** | `E:\03_Devops\NodeJS\` | Prepend to PATH: `$env:Path = "E:\03_Devops\NodeJS;" + $env:Path` |
| **Flutter SDK** | `E:\03_Devops\flutter\bin\flutter.bat` | Prepend to PATH: `$env:Path = "E:\03_Devops\flutter\bin;" + $env:Path` |
| **ffmpeg / ffprobe** | `E:\03_Devops\hermes\tools\ffmpeg-9.0.1-win32-x64\bin\` | Invoke directly for test fixture generation |
| **Rclone** | `E:\03_Devops\rclone\rclone.exe` | Remotes: `gdrive:`, `CMX:`. Omit stderr redirects in PowerShell |

## 2. PowerShell 5.1 Quoting & Binary Capture Safeguards

1. **Multiline Git Commit Messages**:
   - `git commit -m @"..."@` splits arguments when messages contain double quotes, causing commit failures and staging pollution.
   - **Rule**: Write commit messages to a scratch file and commit via `git commit -F <file>`.
2. **Binary ADB Screenshots**:
   - Do NOT run `adb exec-out screencap -p > file.png` from PowerShell (stdout gets re-encoded as UTF-16 text).
   - **Rule**: Run `adb shell screencap -p /sdcard/snap.png` followed by `adb pull /sdcard/snap.png file.png`.
3. **SSH Remote Script Piping**:
   - PowerShell 5.1 prepends a Byte Order Mark (BOM) when piping scripts to `ssh`.
   - **Rule**: Use `cmd /c "ssh homelab bash -s < script.sh"`.
