# Portable Windows Native Toolchain Troubleshooting & Technical Notes

This reference provides solutions to issues encountered when configuring native toolchains in non-standard drives without administrator privileges.

---

## 1. Rust C++ Dependencies & C Runtime (CRT) Conflicts
- **Symptom**: Unresolved external symbol `__imp__CrtDbgReport` or conflicts between `MSVCRT.lib` and `MSVCRTD.lib` / `LIBCMT.lib`.
- **Cause**: Packages such as `boring-sys` or `openssl-sys` compile native C/C++ libraries with release runtime configurations by default (`/MD`), while a Rust debug build (`cargo build`) may attempt to link debug CRT symbols.
- **Resolution**:
  - Target release builds (`cargo build --release` or `npm run tauri build`).
  - Or explicitly configure CRT linkage in Cargo config / crate build flags.

---

## 2. Bindgen & `libclang` Discovery
- **Symptom**: `thread 'main' panicked at ... Unable to find libclang: "couldn't find any valid shared libraries matching: ['clang.dll', 'libclang.dll']"`.
- **Cause**: Rust bindgen searches system PATH and `LIBCLANG_PATH` for `libclang.dll`.
- **Resolution**:
  - Install prebuilt wheels via Python: `pip install libclang`.
  - Set `LIBCLANG_PATH` pointing directly to `<PythonDir>\Lib\site-packages\clang\native`.
  - Ensure `<PythonDir>\Lib\site-packages\clang\native` is added to `PATH`.

---

## 3. MSVC & Windows SDK Administrative Extraction
- **Symptom**: Visual Studio Installer requires administrator privileges and system-level installation.
- **Cause**: Standard installers write to `C:\Program Files` and register system services/registry keys.
- **Resolution**:
  - Download official `.msi` and payload `.zip` packages from Visual Studio manifest endpoints.
  - Unpack `.msi` packages using `msiexec /a package.msi /quiet /qn TARGETDIR=<DevOpsDir>\msvc`.
  - The `/a` switch performs an administrative network extraction without installing to the system or requiring UAC.

---

## 4. Drive Volume Traversal in Batch Files
- **Symptom**: `cd E:\folder` does not change drive from `C:`, causing subsequent build commands to run in the user's home folder.
- **Resolution**:
  - Always use `cd /d "%~dp0"` or `cd /d <path>` when executing scripts across multiple drive letters.

---

## 5. Tauri v2 Build Exits with Code 1 on Missing Private Signing Key
- **Symptom**: `Error A public key has been found, but no private key. Make sure to set TAURI_SIGNING_PRIVATE_KEY environment variable.`
- **Cause**: Tauri finishes compiling and packaging the complete NSIS setup installer, then attempts to create and sign updater `.zip` artifacts because `createUpdaterArtifacts: true` is configured in `tauri.conf.json`.
- **Resolution**:
  - The setup installer (`Antigravity Tools_<Version>_<Arch>-setup.exe`) in `target\release\bundle\nsis\` is already fully created and usable.
  - To prevent the exit code error during non-release or local testing, provide `TAURI_SIGNING_PRIVATE_KEY` or set `"createUpdaterArtifacts": false`.

