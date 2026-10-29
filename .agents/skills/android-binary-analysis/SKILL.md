---
name: android-binary-analysis
description: Unpack, decompile, and audit Android packages (.apk, .xapk, split APKs), mapping Dalvik bytecode and JNI bindings to ARM64/x86_64 native ELF shared objects.
---

# Android Binary & Native Runtime Analysis Skill

Use this skill when auditing Android applications, reverse engineering native libraries, or mapping JNI bindings.

## 1. Storage & Tool Locations
- Analysis workspace: Always create projects under `E:\02_Projects\<target_name>`.
- Toolchain paths:
  - JADX: `E:\03_Devops\jadx\bin\jadx.bat`
  - Android NDK LLVM: `E:\03_Devops\AndroidSdk\ndk\<version>\toolchains\llvm\prebuilt\windows-x86_64\bin\`
    - Binaries: `llvm-readelf.exe`, `llvm-objdump.exe`, `llvm-strings.exe`, `llvm-nm.exe`

## 2. Package Extraction Protocol
1. **XAPK / APKS Extraction**:
   Use .NET's `ZipFile` assembly rather than `Expand-Archive`:
   ```powershell
   Add-Type -AssemblyName System.IO.Compression.FileSystem
   [System.IO.Compression.ZipFile]::ExtractToDirectory($xapkPath, "$workDir\extracted")
   ```
2. **Split APK Architecture Identification**:
   - `base.apk`: Contains `classes.dex`, manifest, assets, fonts, layouts.
   - `config.<abi>.apk`: Contains native `.so` binaries for target ABI (`arm64-v8a`, `armeabi-v7a`, `x86_64`).
   - Extract `.so` files into dedicated ABI subdirectories for isolated static analysis.

## 3. Bytecode Decompilation (Headless JADX)
Run JADX as a background task without decompiling heavy UI resources if only reviewing code:
```powershell
& "E:\03_Devops\jadx\bin\jadx.bat" -d "$workDir\jadx_out" --show-bad-code --no-res "$baseApkPath"
```
Monitor progress via reactive task wakeups.

## 4. Native ELF Auditing (ARM64 / x86_64)
1. **Dynamic Headers & Dependencies**:
   ```powershell
   & $readelf -h $soPath
   & $readelf -d $soPath
   ```
2. **Exported JNI Function Table**:
   ```powershell
   & $readelf -s --dyn-syms $soPath | Select-String "Java_"
   ```
3. **Disassembly & Cross-Referencing**:
   Correlate JNI function signatures (`Java_<package>_<class>_<method>`) with Java `native` declarations in JADX.
   Disassemble specific target functions:
   ```powershell
   & $objdump -d --start-address=0x<start> --stop-address=0x<end> $soPath
   ```
4. **In-Memory Struct Offset Tracing**:
   Trace pointer registers (e.g. `x0`, `x19`, `x20` on ARM64) across memory loads (`ldr`, `ldrb`, `str`, `strb`) to map class fields and state structs.

## 5. Dynamic Asset & CDN Reconnaissance Protocol
When auditing applications that download external packages, models, or dictionaries:

1. **`resources.arsc` Dynamic Endpoint Resolution**:
   - Compilers frequently externalize server URLs into `resources.arsc`.
   - Parse the global string pool in `resources.arsc` to locate endpoints (e.g. `anfilelist.php`, `bundle_server_url`, `file_server_url`).
   - Map string pool indices back to package `ResTable_type` chunks (`0x0201`) to uncover the exact `R.string.<name>` symbol and cross-reference its callers in JADX.
2. **Secondary Manifest & Model Discovery**:
   - Inspect `assets/manifest.json` or embedded config descriptors for third-party CDN endpoints (e.g. Google Digital Ink, ML Kit, PaddleOCR).
3. **High-Volume Multithreaded Archiving**:
   - Deduplicate download URLs from parsed XML/JSON catalogs.
   - Use `concurrent.futures.ThreadPoolExecutor` (8–16 workers) with exponential retry backoff.
   - **Checksum Validation**: Compute and verify catalog-provided checksums (e.g. 32-bit CRC32 for Palm PDB zips, MD5 for neural models) and byte lengths on all landing files.

