---
name: windows-portable-build-tools
description: >-
  Use this skill when tasked with installing, configuring, or repairing native build toolchains
  (MSVC, Windows SDK, Rust, Cargo, Node.js, NASM, CMake, Ninja, Python, libclang, JDK 21, Ghidra, PyGhidra MCP)
  on Windows without administrator privileges or when installing to custom drives or isolated folders.
---

# Windows Portable Native Build Tools Workflow

This skill provides procedures for setting up a fully self-contained, portable native development and compilation toolchain on Windows without administrative privileges or UAC elevation.

It enables building complex desktop and native applications (such as Tauri v2, Rust native extensions, BoringSSL, CMake, and C/C++ projects) and binary analysis suites (Ghidra, PyGhidra MCP) entirely within an arbitrary target directory (e.g. `E:\03_Devops` or `%USERPROFILE%\devops`).

---

## 1. Directory Structure

A standardized portable toolchain layout:

```text
<DevOpsDir>/
├── Cargo/             # CARGO_HOME (bins, registry, git checkouts)
├── Rustup/            # RUSTUP_HOME (toolchains, e.g. stable-x86_64-pc-windows-msvc)
├── NodeJS/            # Extracted Node.js portable zip (node.exe, npm.cmd)
├── Git/               # MinGit or Git portable (cmd/git.exe)
├── msvc/              # Portable MSVC and Windows SDK
│   ├── VC/Tools/MSVC/<version>/
│   └── Windows Kits/10/
├── NASM/              # nasm.exe
├── CMake/             # bin/cmake.exe
├── Ninja/             # ninja.exe
├── JDK/               # OpenJDK 21 LTS (Temurin / Adoptium)
├── Ghidra/            # NSA Ghidra SRE Suite (ghidraRun.bat)
├── Python/            # Portable Python or embeddable python.exe
│   ├── Scripts/       # pip, pyghidra-mcp.exe, pyghidra.exe
│   └── Lib/site-packages/clang/native/libclang.dll
├── env.bat            # Activator script for cmd.exe
└── env.ps1            # Activator script for PowerShell
```

---

## 2. Step-by-Step Toolchain Installation

### A. Portable Git
Download MinGit (or portable Git) and extract without installer:
```powershell
curl.exe -fSLo mingit.zip https://github.com/git-for-windows/git/releases/download/v2.48.1.windows.1/MinGit-2.48.1-64-bit.zip
tar.exe -xf mingit.zip -C <DevOpsDir>\Git
```

### B. Portable Node.js & npm
Download Node.js Windows zip and extract:
```powershell
curl.exe -fSLo node.zip https://nodejs.org/dist/v24.21.0/node-v24.21.0-win-x64.zip
tar.exe -xf node.zip -C <DevOpsDir>
Move-Item "<DevOpsDir>\node-v24.21.0-win-x64" "<DevOpsDir>\NodeJS"
```

#### Package Managers (pnpm, yarn)
Ensure `npm config get prefix` resolves to `<DevOpsDir>\NodeJS`. Install global package managers directly into the portable toolchain:
```powershell
npm install -g pnpm yarn
```
Executable scripts (`pnpm.cmd`, `pnpm.ps1`, `yarn.cmd`) are placed directly in `<DevOpsDir>\NodeJS` and inherit the User `PATH`.

### C. Portable MSVC & Windows SDK (No Admin / No VS Installer)
Use [setup_portable_msvc.py](./scripts/setup_portable_msvc.py) which downloads official MSVC packages from Microsoft CDNs and unpacks them via `msiexec /a` into the destination folder without registering with Windows or requiring admin rights:
```powershell
python ./scripts/setup_portable_msvc.py --output <DevOpsDir>\msvc --accept-license
```

### D. Portable Rust & Cargo
Redirect toolchain and package stores before running `rustup-init.exe`:
```cmd
set "RUSTUP_HOME=<DevOpsDir>\Rustup"
set "CARGO_HOME=<DevOpsDir>\Cargo"
curl.exe -fSLo rustup-init.exe https://win.rustup.rs/x86_64
rustup-init.exe -y --no-modify-path --default-toolchain stable-x86_64-pc-windows-msvc
```

### E. Build Utilities: NASM, CMake, Ninja
Extract portable zips directly into their respective subdirectories:
- **NASM**: `https://www.nasm.us/pub/nasm/releasebuilds/.../win64/nasm-...-win64.zip`
- **CMake**: `https://github.com/Kitware/CMake/releases/download/.../cmake-...-windows-x86_64.zip`
- **Ninja**: `https://github.com/ninja-build/ninja/releases/download/.../ninja-win.zip`

### F. libclang (for Rust `bindgen`)
Install `libclang` into Python or download LLVM prebuilt binaries:
```cmd
<DevOpsDir>\Python\python.exe -m pip install libclang
```
Set `LIBCLANG_PATH` to `<DevOpsDir>\Python\Lib\site-packages\clang\native`.

### G. Portable OpenJDK 21 LTS (for Ghidra & PyGhidra)
Download Eclipse Adoptium Temurin 21 (or Microsoft OpenJDK 21) Windows x64 zip:
```powershell
curl.exe -fSLo jdk21.zip https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.12.1%2B1/OpenJDK21U-jdk_x64_windows_hotspot_21.0.12.1_1.zip
Expand-Archive -Path jdk21.zip -DestinationPath <DevOpsDir>\downloads\jdk_extracted
Move-Item "<DevOpsDir>\downloads\jdk_extracted\*" "<DevOpsDir>\JDK"
```
Set `JAVA_HOME` to `<DevOpsDir>\JDK` and prepend `<DevOpsDir>\JDK\bin` to `PATH`.

### H. Portable Ghidra & PyGhidra MCP
1. Download Ghidra release zip from GitHub:
```powershell
curl.exe -fSLo ghidra.zip https://github.com/NationalSecurityAgency/ghidra/releases/download/Ghidra_12.1.4_build/ghidra_12.1.4_PUBLIC_20260921.zip
tar.exe -xf ghidra.zip -C <DevOpsDir>\downloads\ghidra_extracted
Move-Item "<DevOpsDir>\downloads\ghidra_extracted\*" "<DevOpsDir>\Ghidra"
```
2. Set `GHIDRA_INSTALL_DIR` to `<DevOpsDir>\Ghidra` and add to `PATH`.
3. Install PyGhidra and PyGhidra MCP into Python:
```powershell
<DevOpsDir>\Python\python.exe -m pip install pyghidra-mcp
```
4. Register the MCP server in `~/.gemini/config/mcp_config.json`:
```json
{
  "mcpServers": {
    "pyghidra": {
      "command": "<DevOpsDir>\\Python\\Scripts\\pyghidra-mcp.exe",
      "args": ["-t", "stdio"],
      "env": {
        "JAVA_HOME": "<DevOpsDir>\\JDK",
        "GHIDRA_INSTALL_DIR": "<DevOpsDir>\\Ghidra"
      }
    }
  }
}
```

### I. Deploying Native Tauri / Rust Binaries as CLI & Stdio MCP Servers
Modern native desktop tools (such as Tauri v2 / Rust applications) often bundle command-line tools and an asynchronous stdio JSON-RPC Model Context Protocol (MCP) server directly inside the core binary (e.g. `<app>.exe mcp`).
1. Deploy the compiled binary to `<DevOpsDir>\<AppName>\bin\<app>.exe`.
2. Copy companion Windows runtime libraries (`WebView2Loader.dll`) and assets into `<DevOpsDir>\<AppName>\bin\`.
3. Add `<DevOpsDir>\<AppName>\bin` to the User `PATH` ahead of `WindowsApps`.
4. Register the MCP server in `~/.gemini/config/mcp_config.json`:
```json
{
  "mcpServers": {
    "<app-name>": {
      "command": "<DevOpsDir>\\<AppName>\\bin\\<app>.exe",
      "args": ["mcp"]
    }
  }
}
```

---

## 3. Environment Activation Scripts

Use the preconfigured template in [template_env.bat](./scripts/template_env.bat) or create `env.bat`:
```cmd
@echo off
set "DEVOPS_ROOT=E:\03_Devops"

set "RUSTUP_HOME=%DEVOPS_ROOT%\Rustup"
set "CARGO_HOME=%DEVOPS_ROOT%\Cargo"
set "LIBCLANG_PATH=%DEVOPS_ROOT%\Python\Lib\site-packages\clang\native"
set "JAVA_HOME=%DEVOPS_ROOT%\JDK"
set "GHIDRA_INSTALL_DIR=%DEVOPS_ROOT%\Ghidra"

set "PATH=%CARGO_HOME%\bin;%DEVOPS_ROOT%\NodeJS;%DEVOPS_ROOT%\Git\cmd;%DEVOPS_ROOT%\CMake\bin;%DEVOPS_ROOT%\Ninja;%DEVOPS_ROOT%\NASM;%DEVOPS_ROOT%\Python;%DEVOPS_ROOT%\Python\Scripts;%DEVOPS_ROOT%\JDK\bin;%DEVOPS_ROOT%\Ghidra;%PATH%"

call "%DEVOPS_ROOT%\msvc\setup.bat"
```

---

## 4. Key Rules and Troubleshooting Invariants

1. **Release Mode for C/C++ CRT Linking (`--release`)**:
   Rust crates linking native C++ libraries (such as `boring-sys` / BoringSSL) compile with release runtime references by default. In debug mode, native libraries may reference `_CrtDbgReport` (`ucrtd.lib`), causing linker conflicts. Build with `--release` or configure CRT linkage explicitly.
2. **Path Separators & Drive Switching**:
   Always execute builds using `cd /d "%~dp0"` or `cd /d <TargetDir>` when scripts operate across drive boundaries (e.g. `C:` to `E:`).
3. **PowerShell Inline Escaping**:
   Execute batch files with `cmd.exe /c "call env.bat && ..."` rather than complex PowerShell string interpolation to protect environment variables from premature evaluation.
4. **WindowsApps Execution Alias Shadowing (`python.exe` pitfall)**:
   Windows installs placeholder execution aliases in `%LOCALAPPDATA%\Microsoft\WindowsApps` (e.g. `python.exe`) that redirect to Microsoft Store. When adding tools to User `PATH`, always place `<DevOpsDir>\Python` and `<DevOpsDir>\Python\Scripts` **before** the WindowsApps directory in the PATH order.
5. **Tauri v2 NSIS Installer Bundling & Updater Signing Caveat**:
   Running `npm run tauri build -- --bundles nsis` will generate the NSIS setup installer at `src-tauri\target\release\bundle\nsis\<Name>_<Version>_<Arch>-setup.exe`. If `createUpdaterArtifacts: true` is configured in `tauri.conf.json` without `TAURI_SIGNING_PRIVATE_KEY` set in the environment, the process exits with code 1 *after* the NSIS `.exe` has already been built. Verify the output directory before treating the build as failed.
6. **Electron-Builder Windows Targets & Local Offline EXE Packaging**:
   - Default configurations often specify `nsis-web`, which attempts to fetch web packages from remote GitHub releases at install time. For local standalone builds (`-p never`), explicitly target `nsis` (offline installer) and/or `portable` (standalone executable).
   - Specify architecture explicitly (`--x64`) to prevent downloading multi-architecture Electron binaries (`ia32`, `arm64`) unnecessarily.
   - Supply targets as positional arguments rather than repeating `-c`:
     ```powershell
     pnpm electron-builder --win nsis portable --x64 -p never
     ```
7. **PyGhidra MCP Windows File Lock Contention (`LockException` / `CONNECTION_CLOSED`)**:
   - **Symptom**: PyGhidra MCP fails to reconnect (`CONNECTION_CLOSED`), and headless logs report `LockException: Unable to lock project! <path>`. Running `Remove-Item *.lock*` in PowerShell fails with `IOException: The process cannot access the file because it is being used by another process`.
   - **Cause**: When an MCP client (Claude Code, IDE) disconnects abruptly or terminates unexpectedly, orphaned background `python.exe` or `pyghidra-mcp.exe` processes retain open Win32 handles on `<project_name>.lock~`.
   - **Resolution Procedure**:
     1. Terminate orphaned PyGhidra MCP processes using Python/`psutil`:
        ```powershell
        python -c "import psutil, os; [p.kill() for p in psutil.process_iter(['pid', 'name', 'cmdline']) if p.info['cmdline'] and any('pyghidra' in x for x in p.info['cmdline']) and p.pid != os.getpid()]"
        ```
     2. Remove all stale `.lock` and `.lock~` files from the project directory:
        ```powershell
        Remove-Item "<project_path>\*.lock*" -Force
        ```
     3. Restart the MCP client or re-initialize the connection.


