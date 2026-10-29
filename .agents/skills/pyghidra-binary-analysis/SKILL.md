---
name: pyghidra-binary-analysis
description: >-
  Reverse engineer native shared libraries (.so, .dll, .elf), inspect decompiled C code,
  and analyze in-memory dumps using PyGhidra MCP tools. Use when reverse engineering game binaries,
  unpacking obfuscated libraries, or extracting protocol offsets.
---

# PyGhidra Binary Analysis Workflow

## Overview
Antigravity integrates native PyGhidra MCP tools (`pyghidra` server) for headless Ghidra analysis of native binaries and decrypted runtime dumps.

## 1. Handling Packed & Encrypted Binaries (`libg.so`)
- **Root Problem**: On-disk `.so` files often use runtime packers/decrypters. Loading on-disk files into Ghidra reveals only the stub loader with encrypted payloads and missing symbols.
- **In-Memory Dump Workflow**:
  1. Execute process under emulator/QEMU or attach Frida.
  2. Wait for native loader initialization to complete.
  3. Dump memory range (`/proc/<pid>/maps` or Frida `Memory.readByteArray`).
  4. Fix ELF header / program headers if dumped from memory.
  5. Import the dumped memory binary into PyGhidra.

## 2. PyGhidra MCP Tool Sequence
- **Step 1: Import**:
  Use `call_mcp_tool(ServerName='pyghidra', ToolName='import_binary', Arguments={'file_path': 'E:\\path\\to\\dump.so'})`.
- **Step 2: Reconnaissance**:
  - List exports: `list_exports`
  - List imports: `list_imports`
  - Search string constants: `search_strings(query='...', min_length=5)`
  - Find symbols: `search_symbols_by_name(query='...')`
- **Step 3: Decompilation & Cross-References**:
  - Decompile candidate functions: `decompile_function(name_or_address='...')`
  - Find callers/callees: `list_xrefs(address='...')` or `gen_callgraph(entry_point='...')`
- **Step 4: Annotate & Save**:
  - Rename obfuscated symbols: `rename_function`, `rename_variable`
  - Set accurate signatures: `set_function_prototype`
  - Save Ghidra project database: `save`

## 3. Native PyGhidra on Linux (no MCP — verified recipe, PyGhidra 3.1 / Ghidra 12.1)

The MCP tool names above belong to the Antigravity harness. For direct Python work on the Linux side:

- `pyghidra.analyze_program` does NOT exist in 3.1.x — do not import it.
- `GHIDRA_INSTALL_DIR=/opt/ghidra` must be in the environment (pacman ghidra installs to `/opt/ghidra`); without it the launcher dies with "GHIDRA_INSTALL_DIR is not set".
- Verified import+analyze+count recipe (`APK/libg.so` → 916 functions):

```python
import warnings; warnings.filterwarnings("ignore", category=DeprecationWarning)
from pyghidra import open_program   # deprecated-but-working generator API
with open_program("APK/libg.so", project_location=out_dir, analyze=True) as prog:
    n = 0; f = prog.getFirstFunction()
    while f is not None:
        n += 1; f = prog.getFunctionAfter(f)
```

- Pitfall: `open_program` yields a `FlatProgramAPI`, NOT a `Program` — no `getFunctionManager()`/`getProgram()`/`getEntryPoint()` on it. Walk functions with `getFirstFunction()` + `getFunctionAfter()`; introspect with `dir(prog)` before guessing members.
- `api.program_loader()` (ProgramLoader.builder) has no `binaryPath` method in Ghidra 12.1 — use `open_program`.
- Offset portability: `libndk_translation` patch offsets are build-specific. The ARM guest build (`scratch/guest_libndk.so`, 1,272,208 B) holds the `0f 85 94 00 00 00` jcc at `0xde718`; the x86_64 api30 build (`scratch/api30_libndk.so`, 1,654,288 B) holds those same bytes at `0x110538` (`WrapGuestFunctionImpl`) and unrelated bytes (`d0 0d 66 0f 3a 14`) at `0xde718`. Always confirm the 6-byte pattern at the target offset in the exact file being patched — never reuse a patched `.so` across builds.
