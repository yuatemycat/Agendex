---
name: portable-ndk-build-system
description: >-
  Procedures for portable Android NDK and LLVM toolchain discovery, dynamic repository root derivation,
  and mandatory dual-flavor tandem compilation (dev and customer) with Ed25519 request signing enforcement,
  sourceless AST-stripped bytecode distribution, and packaging in CUSTOMER_BUILDS/.
---

# Portable Android NDK Build System & Dual-Flavor Distribution

This skill outlines the engineering standards and build toolchain practices for compiling in-guest ARM64 native hooks (`libhdguest-hook.so`) and packaging release distributions across disparate host environments without hardcoded developer paths or flavor divergence.

---

## 1. Portable Repository Root & Toolchain Discovery

### 1.1 Dynamic Repository Root Derivation
Never hardcode host-specific drive letters or machine profiles (e.g. `D:\02_Projects\...` or `C:\Users\<user>\...`). Always derive the project root relative to the executing script:
```python
from pathlib import Path

# In instrumentation/houdini/scripts/build_hook.py:
ROOT_DIR = Path(__file__).resolve().parents[3]
```

### 1.2 Multi-Tier NDK & Clang Discovery
Developer machines often store the Android NDK in varying SDK locations or separate DevOps drives (e.g. `E:\03_Devops\AndroidSdk\ndk`). Toolchains must dynamically discover the newest installed NDK that actually contains the `windows-x86_64` LLVM/Clang binary:

```python
import os
from pathlib import Path

def discover_ndk() -> Path:
    """Finds an NDK installation containing the clang++ compiler."""
    rel = Path("toolchains") / "llvm" / "prebuilt" / "windows-x86_64" / "bin" / "clang++.exe"
    
    # 1. Check explicit environment variables
    for env in ("ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "NDK_HOME"):
        v = os.environ.get(env)
        if v and (Path(v) / rel).exists():
            return Path(v)
            
    # 2. Check candidate SDK directories
    candidates = []
    sdk = os.environ.get("ANDROID_SDK_ROOT") or os.environ.get("ANDROID_HOME")
    if sdk:
        candidates.append(Path(sdk) / "ndk")
    candidates += [
        Path(r"E:\03_Devops\AndroidSdk\ndk"),
        Path.home() / "AppData" / "Local" / "Android" / "Sdk" / "ndk",
    ]
    
    for base in candidates:
        if base.is_dir():
            # Sort versioned directories descending (e.g., 27.x.x > 26.x.x)
            for d in sorted((p for p in base.iterdir() if p.is_dir()), reverse=True):
                if (d / rel).exists():
                    return d
                    
    raise RuntimeError("No valid Android NDK with windows-x86_64 clang++ found.")
```

---

## 2. Mandatory Dual-Flavor Tandem Updates (Rule 0.5)

Whenever introducing new hook opcodes, protocol logic, or bug fixes, **both `dev` and `customer` flavors must be updated and compiled in tandem**. Divergence between flavors is strictly forbidden.

### 2.1 Flavor Domain Boundaries
| Aspect | Dev Flavor (`dev.so` / `libhdguest-hook.so`) | Customer Flavor (`customer.so`) |
| :--- | :--- | :--- |
| **Debug Symbols** | Full unstripped debug symbols | Completely stripped (`-s`, `-DHD_CUSTOMER_BUILD`) |
| **Authentication** | Unauthenticated local IPC enabled | **Mandatory Ed25519 signature enforcement** (`-DHD_REQUIRE_SIGNED_REQUESTS`) |
| **Canary Token** | None | Must embed `HD_CUSTOMER_ENFORCED_ED25519` |
| **Transport** | Abstract socket + file polling fallback | **Pure socket-only** (`\0sys_render_ipc`); file polling stripped |
| **Simulation Fold** | Local $f_1$ checksum fold executed in `protocol_core.pyd` | Local $f_1$ fold omitted; calls remote jobs service (`seal()`) |
| **Wire Protocol** | Native wire action numbers | Synthetic obfuscated tags (`900001+` via `WIRE_TO_TAG`) |
| **Source Delivery** | Readable `.py` Python source files | **Sourceless `.pyc` bytecode distributions** with AST docstrings stripped |
| **Exceptions** | Raw diagnostic tracebacks | Masked opaque reference codes (`"Reference: <HEX>"`) |

### 2.2 Compilation Invariant & Verification
The customer hook must enforce Ed25519 request verification at compile time:
```cpp
#if defined(HD_CUSTOMER_BUILD) && !defined(HD_REQUIRE_SIGNED_REQUESTS)
#error "Customer build MUST define HD_REQUIRE_SIGNED_REQUESTS"
#endif
```
Fail-closed authentication: Unauthenticated requests without a valid session token or Ed25519 signature must immediately return `rc=-99` or `rc=-97`. The hook must never fail open.

### 2.3 Strict Device Permissions Baseline
- Any directory created on the Android device (e.g., `/data/local/tmp/.sys_cache/`) must use `0700` (`S_IRWXU`) permissions.
- All IPC sockets, temporary buffers, and telemetry dumps must use `0600` (`S_IRUSR | S_IWUSR`). Wide-open modes (`0777`, `0666`) are strictly banned.

---

## 3. Customer Packaging Pipeline (`package_customer_builds.py`)

When releasing updates to `CUSTOMER_BUILDS/`, execute the synchronized packaging pipeline:

### 3.1 Build Commands
```powershell
# 1. Compile both native hook flavors in tandem
python instrumentation/houdini/scripts/build_hook.py --flavor dev
python instrumentation/houdini/scripts/build_hook.py --flavor customer

# 2. Package and synchronize customer distributions
python tools/packaging/package_customer_builds.py
```

### 3.2 Automated Validation Gates
The packaging pipeline enforces strict pre-flight gates:
1. **Sourceless Bytecode Verification**: Confirms no plaintext `.py` source files or developer docstrings remain in customer distribution packages.
2. **Binary Canary Audit**: Verifies `customer.so` contains the `HD_CUSTOMER_ENFORCED_ED25519` symbol.
3. **Secrecy Audit**: Asserts that `configs/` and fleet credentials (`accounts/*.json`) are never bundled. Only public 32-byte Ed25519 verification keys are embedded.
4. **Preflight Gating**: Automatically archives the preflighted tree to `CUSTOMER_BUILDS/` rather than the ungated staging directory.
