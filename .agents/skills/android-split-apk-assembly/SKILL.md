---
name: android-split-apk-assembly
description: >-
  Procedures for inspecting Android App Bundle / XAPK / APKM SDK version requirements (minSdk),
  verifying cryptographic signing parity across disparate split sources, installing split bundles
  via adb install-multiple, and headlessly extracting assets/fingerprints from asset packs.
---

# Android Split APK Assembly & Headless Asset Pipeline

This skill provides the authoritative runbook for handling modern multi-split Android App Bundles (`.xapk`, `.apkm`, `.apks`) when targeting Android emulators with restricted API versions (such as MEmu Android 9 / API 28) and headlessly extracting game assets without emulator dependency.

---

## 1. SDK Pre-Flight Inspection

When modern game updates fail on older emulators with:
```text
Failure [INSTALL_FAILED_OLDER_SDK: Requires newer sdk version #X (current version is #Y)]
```

Always inspect `minSdkVersion` / `sdkVersion` across all candidate packages before attempting install:

### Inspect via `aapt`
```powershell
& "<memu_or_sdk_path>\aapt.exe" dump badging "<package.apk>" | Select-String "sdkVersion"
```

### Inspect via Python Binary XML Parser
```python
import struct, zipfile

def get_sdk_versions(apk_path):
    with zipfile.ZipFile(apk_path, 'r') as z:
        data = z.read('AndroidManifest.xml')
    # minSdkVersion is attribute with name 0x0101020c
    # In Supercell binary manifests, uses-sdk tag holds min and target SDK values
```

### Known Distribution Discrepancies
- **APKPure XAPK**: Often packages splits generated for newer API baselines (e.g., `minSdkVersion = 29` for 64-bit arm64-v8a or `32` for 32-bit armeabi-v7a).
- **APKMirror APKM**: Frequently carries universal base splits (`minSdkVersion = 24`, Android 7.0+) covering all architectures, but often omits large install-time asset packs (`install_time_asset_pack.apk`).

---

## 2. Cryptographic Certificate Matching Across Disparate Splits

When pairing a `base.apk` from one provider (e.g. APKMirror) with an `install_time_asset_pack.apk` from another (e.g. APKPure), **both must share the exact same cryptographic signing key**.

Android's package installer strictly verifies that all splits in a multi-split session share the same signer:

```python
import struct, hashlib

def get_apk_v2_cert_sha256(apk_path: str) -> str:
    """Extracts the SHA-256 fingerprint of the signing certificate from APK Signing Block (v2/v3)."""
    with open(apk_path, 'rb') as f:
        f.seek(-min(65536, f.seek(0, 2)), 2)
        data = f.read()
        eocd_pos = data.rfind(b'PK\x05\x06')
        cd_offset = struct.unpack_from('<I', data, eocd_pos + 16)[0]
        f.seek(cd_offset - 24)
        magic_block = f.read(24)
        size_of_block = struct.unpack_from('<Q', magic_block, 0)[0]
        if magic_block[8:24] != b'APK Sig Block 42':
            return None
        f.seek(cd_offset - size_of_block - 8)
        block = f.read(size_of_block)
        pos = 8
        pairs = {}
        while pos < len(block) - 24:
            p_len = struct.unpack_from('<Q', block, pos)[0]
            pos += 8
            p_id = struct.unpack_from('<I', block, pos)[0]
            pairs[p_id] = block[pos+4:pos+p_len]
            pos += p_len
        v2 = pairs.get(0x7109871a) or pairs.get(0xf05368c0)
        if not v2: return None
        idx = v2.find(b'\x30\x82')
        if idx != -1:
            c_len = struct.unpack_from('>H', v2, idx+2)[0] + 4
            return hashlib.sha256(v2[idx:idx+c_len]).hexdigest()
    return None
```

- If `get_apk_v2_cert_sha256(base_apk) == get_apk_v2_cert_sha256(asset_apk)`, the splits can be committed safely together in one session.

---

## 3. Atomic Multi-Split Installation

Once verified, stream and commit all splits simultaneously via ADB:

```powershell
adb -s <serial> install-multiple -r -d -g `
    "path\base.apk" `
    "path\split_config.<arch>.apk" `
    "path\split_config.en.apk" `
    "path\split_config.<dpi>.apk" `
    "path\install_time_asset_pack.apk"
```

Verify installed status:
```powershell
adb -s <serial> shell "pm list packages -f com.supercell.hayday; dumpsys package com.supercell.hayday | grep splits"
```
Expected output:
```text
splits=[base, config.<arch>, config.en, config.<dpi>, install_time_asset_pack]
```

---

## 4. Headless Asset & Fingerprint Extraction (Zero-Emulator Dependency)

Pure-TCP protocol tools do not require emulator execution to acquire updated game parameters and master hashes:

```python
import zipfile, json

def extract_game_fingerprint(asset_pack_path: str):
    with zipfile.ZipFile(asset_pack_path, 'r') as z:
        data = json.loads(z.read('assets/fingerprint.json').decode('utf-8'))
        print('Version:', data.get('version'))
        print('Master Hash (sha):', data.get('sha'))
        print('Total Assets:', len(data.get('files', [])))
        return data
```

### Files to Update Upon Version Bump:
1. `tools/protocol/tcp_newspaper_sniper.py`: Update `MASTER_HASH`.
2. `tools/protocol/sc_tcp_client.py`: Update `MASTER_HASH` and ClientHello version tuple `(3, 43, 1, <major>, <minor>)`.
3. `tools/protocol/full_server_query.py`: Update `MASTER_HASH`.

---

## 5. Multi-Emulator ADB Socket Shadowing & Disambiguation

When running multiple Android emulators simultaneously (e.g. MuMu Player and LDPlayer):

### The Socket Binding Conflict
- **MuMu Player**: Binds explicitly to `127.0.0.1:5555` (`Listen` on loopback).
- **LDPlayer 9**: Binds to wildcard `0.0.0.0:5555` (`Listen` on all interfaces).
- **The Shadowing Bug**: In Windows TCP stack, requests to `127.0.0.1:5555` or `localhost:5555` always match the specific loopback socket first. ADB connects to MuMu Player even when targeting LDPlayer.

### Resolution Runbook
1. **Identify Host LAN IP**:
   ```powershell
   Get-NetIPAddress -AddressFamily IPv4 | Where-Object { $_.IPAddress -like "192.168.*" -or $_.IPAddress -like "10.*" }
   ```
2. **Connect via Host Interface**:
   ```powershell
   adb connect <host_lan_ip>:5555  # e.g., adb connect 192.168.1.5:5555
   ```
   This bypasses loopback socket precedence and routes directly to LDPlayer's `0.0.0.0:5555` listener.
3. **LDPlayer ADB Debugging Prerequisite**:
   LDPlayer disables ADB debugging by default (`"basicSettings.adbDebug": 0` in `leidian0.config`). Before attempting connection, verify or set:
   ```json
   "basicSettings.adbDebug": 1
   ```

---

## 6. Canonical Asset Storage & Automated CLI Runner

All packages, splits, and native binaries are maintained under `E:\02_Projects\Intra-farm\APK/`:
- `com.supercell.hayday_*24lang*.apkm`: Universal `minSdk = 24` base & architecture bundle.
- `com.supercell.hayday_*6dpi*.apkm`: Complete distribution bundle with 725 MB asset pack.
- `split_install_time_asset_pack.apk`: Standalone texture asset pack.
- `libg.so` / `libg_arm64-v8a.so` / `libg_armeabi-v7a.so`: Extracted native logic libraries.

### Automated Multi-Split Installer:
[`tools/infrastructure/install_hayday_apkm.py`](file:///E:/02_Projects/Intra-farm/tools/infrastructure/install_hayday_apkm.py) automatically inspects the target device's SDK version (`ro.build.version.sdk`), detects `< 32`, resolves the `minSdk = 24` universal base, streams splits to avoid RAM spikes, and executes `adb install-multiple -r -d -g`:
```powershell
python tools/infrastructure/install_hayday_apkm.py --device <device_serial_or_ip:port>
```

