---
name: antigravity-manager-workflow
description: >-
  Procedures for building, testing, and debugging Antigravity-Manager on Windows.
  Covers MSVC env activation, cargo release testing, Common Controls v6 manifest workarounds,
  token DB safety guards, account deserialization defaults, frontend verification,
  proxy egress fingerprint invariants, process deployment safety, agy SQLite repair,
  Antigravity 2.0 model dropdown resolution runbook, agy.exe 56-byte GenericCommand
  slash command injection, agy_gateway.ps1 argument splatting, and Vertex AI routing.
---

# Antigravity-Manager Development & Test Runbook

## 1. Environment & Rust Toolchain Activation
- Plain shells lack the default rustup toolchain. Always load the MSVC environment:
  `cmd /c "call E:\03_Devops\env.bat >nul && cargo <args>"`
- **Always use `--release`**: The debug profile fails to link the test binary (`boring-sys2` debug CRT unresolved `_CrtDbgReport`).
- Build application binary without updater signing requirement:
  `cmd /c build_exe.bat --no-bundle` (prevents failure on missing `TAURI_SIGNING_PRIVATE_KEY`).

## 2. Unit Testing & Common Controls v6 Manifest Workaround
Running `cargo test` directly results in `STATUS_ENTRYPOINT_NOT_FOUND` due to missing Common Controls v6.
- **Workflow**:
  1. Compile tests without running: `cargo test --release --lib --no-run`
  2. Copy the resulting `target\release\deps\antigravity_tools_lib-*.exe` to an isolated directory.
  3. Ensure a `<test_exe_name>.exe.manifest` declaring `Microsoft.Windows.Common-Controls` 6.0.0.0 exists beside the executable.
  4. Run that executable copy with targeted module filters.
- **Concurrency**: Tests using `TestDataDir` mutate `ABV_DATA_DIR`; run with `--test-threads=1`.
- **Flaky Test**: `modules::account::tests::test_migrate_data_dir_rename_and_copy` has millisecond timestamp collisions that poison `TEST_MUTEX`.

## 3. ⛔ CRITICAL SAFEGUARD: Never Run Live Token DB Tests
- **Hazard**: `modules::user_token_db` tests invoke `init_db()` and `create_token()` against the live database at `~/.antigravity_tools/user_tokens.db`. Running them unfiltered injects fake test tokens into the operator's active proxy and causes 9+ minute test hangs.
- **Rule**: Never run `cargo test` unfiltered. Always filter by module (e.g. `proxy::vertex::`).

## 4. Account Model & Vertex Configuration
- Accounts deserialize with `proxy_disabled = true` by default in this fork; test fixtures require `"proxy_disabled": false` to populate pools.
- Vertex quota project authorization:
  `gcloud auth application-default set-quota-project aurora-503413`
- Frontend typecheck and unit tests:
  Prepend `E:\03_Devops\NodeJS` to PATH, then execute `npx tsc --noEmit` and `npx tsx src/config/__tests__/modelConfig.test.ts`.

## 5. ⛔ PROCESS SAFETY: Never Kill Running Instances Automatically
- **Rule**:
  - **NEVER** run `Stop-Process` or `taskkill` against `agy.exe` — the agent session itself runs inside `agy.exe`, so killing it terminates the active session.
  - Never run `Stop-Process` or `taskkill` against `proxymateur.exe` (`antigravity-tools.exe`) during background builds or edits unless explicitly instructed by the user.
  - For locked Windows executables (`agy.exe`, `language_server.exe`), perform in-place replacement via atomic NTFS rename (`os.rename(exe, f"{exe}.running_{ts}")`) and `shutil.move` without terminating running processes.
- **Deployment Protocol**:
  1. Compile release binary to `target\release\antigravity-tools.exe`.
  2. Provide manual copy and restart commands to the operator:
     ```powershell
     Stop-Process -Name "antigravity-tools" -Force -ErrorAction SilentlyContinue
     Copy-Item -Force "src-tauri\target\release\antigravity-tools.exe" "E:\03_Devops\AntigravityManager\bin\antigravity-tools.exe"
     Copy-Item -Force "src-tauri\target\release\antigravity-tools.exe" "E:\01_Apps\Installed\Antigravity Tools\antigravity-tools.exe"
     Start-Process "E:\03_Devops\AntigravityManager\bin\antigravity-tools.exe"
     ```

## 6. Outbound Egress Invariants (TLS Fingerprint & Header Hygiene)
- **Canonical Emulation**: All outbound HTTP client builders contacting Google endpoints (`oauth2.googleapis.com`, `cloudcode-pa.googleapis.com`, `daily-cloudcode-pa.googleapis.com`) MUST use `crate::utils::http::get_canonical_emulation_option()`.
- **Header Invariants**:
  - **Never leak browser headers**: `rquest` emulation must always have `.skip_headers(true)` enabled to prevent injecting `sec-ch-ua`, `sec-fetch-*`, `upgrade-insecure-requests`, or `accept: text/html` on JSON REST / SSE calls.
  - **OS Matching**: Ensure `EmulationOS` dynamically matches the host platform (`Windows` on Windows, not hardcoded macOS).
  - **Uniform Fingerprint**: Standard clients (OAuth / Quota) and upstream LLM clients must present the identical TLS fingerprint.
  - **Header Sanitization**: Strip incoming downstream client browser headers and internal IDs before forwarding upstream.

## 7. Error 2 (`failed to unmarshal tool call arguments`) Diagnosis & Healing
- **Root Cause**: `agy` (Google Code Assist CLI) is a Go binary. Interrupted streaming writes partial JSON into SQLite (`~/.gemini/antigravity-cli/conversations/<id>.db`). On subsequent turns, `codeassistclient.processToolCalls` executes `json.Unmarshal` on the truncated argument string and permanently crashes with `unexpected end of JSON input`.
- **Healing Architecture**:
  1. **In-Proxy Repair**: `repair_truncated_json_str` in `inbound.rs` auto-closes truncated JSON in message history before upstream dispatch, and `wrapper.rs` auto-closes chunks before `agy` stores them in SQLite.
  2. **Database Repair Script**: If an existing conversation DB is stuck, inspect `messages` and `tool_calls` tables for unclosed braces/quotes, create a `.bak` backup, and patch the JSON to end cleanly with `"\n"}`.

## 8. Automatic Model Fallback Mechanics
- When pool accounts exhaust quota for requested models (e.g. Claude Opus `All accounts limited`), proxy routes to `gemini-3.8-flash`.
- Routing honors `gui_config.json` preference: checks healthy pool accounts first (`accounts_first`), falling back to Vertex AI (`aurora-503413`) if accounts are depleted.

## 9. Antigravity 2.0 "No Models Available" 3-Layer Diagnosis & Resolution Runbook
- **Symptom**: Antigravity 2.0 prompt input displays `Model` -> `No models available` despite healthy account quota.
- **Root Cause (Three Distinct Layers)**:
  1. **Layer 1 (Go Backend Panic in `language_server.exe`)**:
     - `defaults.go:372` panics on startup with `unknown model key MODEL_PLACEHOLDER_M50: model not found` when resolving default model configurations.
     - **Fix**: NOP out the conditional panic branch at file offset `0x1863af8` (`75 3b` -> `90 90`).
  2. **Layer 2 (Model Parser `Hj` in embedded `main.js`)**:
     - `language_server.exe` embeds web assets (`main.js`) in an in-memory Deflate ZIP archive in `.data` (data start `0x84dcedd`, comp size `2,811,063`, uncomp size `9,607,177`).
     - The React model converter `Hj` (offset `1630027` in `main.js`) expects Protobuf class shape `modelOrAlias.choice.case` and accesses `Object.entries(a.supportedMimeTypes)`.
     - Live REST/Connect-RPC responses return proto3 JSON `{ "modelOrAlias": { "model": "..." } }` without `.choice` and with `supportedMimeTypes: undefined`, throwing `TypeError` on all 14 models and emptying `visibleModelConfigs`.
     - **Fix**: Patch `Hj` (454 bytes) in `main.js` to safely resolve `(c?.case==="model"?c.value:m.model)||I.Model.UNSPECIFIED` and default `supportedMimeTypes || {}`. This enables the default button to display `Gemini 3.8 Flash High`.
  3. **Layer 3 (Dropdown Popover Filter `tAb` in embedded `main.js`)**:
     - Even after Layer 1 and Layer 2 are patched, clicking the model dropdown popover still renders `No models available`.
     - In `main.js` at offset `5375717`, `uAb` delegates to `tAb`:
       ```javascript
       const tAb=a=>(0,A.useMemo)(()=>{var b=a.find(c=>c.name.toLowerCase()==="recommended");return b&&b.groups?.length?{groupName:"Recommended",options:b.groups[0]?.options||[]}:{groupName:"Recommended",options:[]}},[a]);
       ```
     - For Enterprise Gateway accounts, the Go backend sets `clientModelSorts[0].name = "Enterprise Gateway Models"`.
     - Because there is no category named `"recommended"`, `b` is `undefined`, returning `{ groupName: "Recommended", options: [] }` (empty options array).
     - Component `FAb` sees `options.length === 0` and renders `<p data-testid="model-selector-empty">No models available</p>`.
     - **Fix**: Patch `tAb` (215 bytes) at offset `5375717` to fallback to `(a || [])[0]` and collect all options via `flatMap`:
       ```javascript
       const tAb=a=>(0,A.useMemo)(()=>{var b=(a||[]).find(c=>c?.name?.toLowerCase()==="recommended")||(a||[])[0];var o=b?.groups?.flatMap(g=>g.options||[])||[];return{groupName:b?.name||"Recommended",options:o}},[a])/***/;
       ```
- **Stream In-Place Replacement & Verification**:
  - Keep uncompressed `main.js` length exact at `9,607,177` bytes.
  - Compress using raw Deflate (level 7) padded with RFC 1951 empty non-compressed blocks (`0x00 0x00 0x00 0xff 0xff`) to match EXACTLY `2,811,063` bytes.
  - Final CRC32 with both Layer 2 and Layer 3 patches: `0xea8c80bf`.
  - Update CRC32 in local header (`0x84dcec6`) and central directory (`0x885a8d0`), writing the stream into `0x84dcedd..0x877b404`.
  - **Windows Locking**: Running `.exe` files are locked for overwrite; use atomic rename (`os.rename(ls_path, old_path)`) before placing the patched binary.
  - **CDP Verification**: Discover Electron WebSocket URL from `language_server.log` (e.g. `ws://127.0.0.1:<port>/devtools/page/...`) and inspect the DOM or capture screenshot via CDP to confirm `model-selector-empty` is null and all 14 models render with reasoning tiers.
- **Consolidated Automated Tools**:
  All automated tooling, verification harnesses, and conversation repair scripts are located in:
  - [`scripts/antigravity_patch/patch_antigravity.py`](file:///E:/02_Projects/Antigravity-Manager/scripts/antigravity_patch/patch_antigravity.py): Automated 3-layer patcher.
  - [`scripts/antigravity_patch/verify_patch.py`](file:///E:/02_Projects/Antigravity-Manager/scripts/antigravity_patch/verify_patch.py): Static and dynamic CDP verification.
  - [`scripts/antigravity_patch/repair_conversations.py`](file:///E:/02_Projects/Antigravity-Manager/scripts/antigravity_patch/repair_conversations.py): SQLite Error 2 repair.
  - [`scripts/antigravity_patch/README.md`](file:///E:/02_Projects/Antigravity-Manager/scripts/antigravity_patch/README.md): Complete architecture documentation.

## 10. `agy.exe` Custom Slash Command Injection (`commands.GenericCommand` 56-Byte ABI & Reference-Point Patcher)
- **Target Binary**: `C:\Users\X\AppData\Local\agy\bin\agy.exe` (Go PE64 binary).
- **Automated Reference-Point Patcher**: [`scripts/auto_patch_agy.py`](file:///E:/02_Projects/Proxymateur/scripts/auto_patch_agy.py) (synced to `E:\03_Devops\AntigravityManager\bin\auto_patch_agy.py` and invoked automatically by `agy_gateway.ps1` on launch).
- **Dynamic Reference-Point Resolution (No Hardcoded Offsets)**:
  - Parses the PE section table dynamically to locate `.text` (`text_base`) and `.rdata`.
  - Scans the binary for the Go `pclntab` magic (`\xf1\xff\xff\xff\x00\x00` / `\xf0\xff\xff\xff\x00\x00`) and resolves all anchors by Go symbol name (`entry_va = text_base + entry_off`):
    - `commands/commands.NewRegistry` (exact function bounds computed via `bisect` to next `pclntab` entry)
    - `commands/commands.(*Registry).Register` (with call-frequency inside `NewRegistry` as stripped-build fallback)
    - `commands/commands.registerExternalCommands` (5-byte `e8 rel32` call site inside `NewRegistry` hooked and preserved as `original_call`)
    - `commands/commands.(*GenericCommand).{Description,Execute,Name}` (used to locate the 24-byte `abi.ITab.Fun` method pointer triple in `.rdata`, `itab_raw = itab_pos - 0x18`, with instruction-pattern scan as fallback)
    - `runtime.morestack` (for Go stack-split prologue inside the shellcode cave)
  - Carves a dedicated `.proxcv` PE section (`0x1000` bytes) for the shellcode (`+0x000`) and command/string table (`+0x400`), stamped with `PROX_CMD_V1`, avoiding any `.rdata` overflow across `agy` upgrades (verified across v1.4.3, v1.4.4, v1.4.5).
- **`commands.GenericCommand` 56-Byte (`0x38`) Struct ABI Invariant**:
  - `*commands.GenericCommand` is **56 bytes (`0x38`)**, NOT 32 bytes:
    - `+0x00` (`16B`): `name string` (`ptr`, `len`)
    - `+0x10` (`16B`): `description string` (`ptr`, `len`)
    - `+0x20` (`24B`): `subcommands []Subcommand` (`ptr`, `len`, `cap`)
  - **Autocomplete Panic Hazard**: Using a 32-byte (`0x20`) stride causes `subcommands` (`+0x20`) of command `N` to overlap with `name` (`+0x00`) of command `N+1`. When `/` autocomplete invokes `GhostHint()` or `GhostCompleteArg()`, it dereferences the next command's ASCII name pointer as a `[]Subcommand` slice and panics.
  - **Fix**: Always allocate `56-byte` (`0x38`) zero-padded entries in the `.proxcv` data cave so `subcommands` (`+0x20..+0x38`) is strictly zero (`nil` slice, `len=0`, `cap=0`).

## 11. `agy_gateway.ps1` PowerShell Splatting & Vertex AI Routing (`gemini-4-argon`)
- **PowerShell 5.1 Single-Argument Splatting Hazard (`agy -c`)**:
  - In `E:\03_Devops\AntigravityManager\bin\agy_gateway.ps1` (and `scripts/agy_gateway.ps1`), returning a 1-element array (`@('-c')`) from `Sync-CliExperimentFlags` or an `if (...) { @(...) }` block unrolls it into a scalar `System.String` (`"-c"`).
  - Splatting `@agyArgs` when `$agyArgs` is a scalar `System.String` splits it into individual characters (`"-"` and `"c"`), breaking `agy -c` with `Error: unexpected argument "-"`.
  - **Invariant**: Always wrap the outer assignment in `@(...)`:
    `$agyArgs = @(if ($rawAgyArgs.Count -gt 0) { Sync-CliExperimentFlags -PassedArgs $rawAgyArgs })`
- **Proxymateur Vertex AI Direct Routing for `gemini-4-argon`**:
  - In `C:\Users\X\.proxymateur\gui_config.json`, `proxy.vertex` must set:
    - `"preference": "vertex_first"` (otherwise `"accounts_first"` routes to the Google OAuth account pool first, which downgrades `gemini-4-argon` to `gemini-3.8-flash-tiered`).
    - `"model_mapping": { "gemini-4-argon": "gemini-4-argon", "gemini-4-argon*": "gemini-4-argon", ... }` so `-high`/`-medium`/`-low` effort variants match in `picker` mode and forward directly to `vertex/gemini-4-argon`.
  - Hot-reload live without restarting `proxymateur.exe` via `POST http://127.0.0.1:8045/api/config` (`{"config": <full_gui_config>}`).
- **Launcher Modes (`agy` vs `agy -px`)**:
  - `agy_gateway.ps1` runs in **native mode by default** (`agy`), bypassing proxy health checks (0ms delay) and stripping leftover `AGY_LLM_GATEWAY*` environment variables.
  - Proxy mode is strictly opt-in via `-px` or `--px` flag (e.g. `agy -px`, `agy -px -c`, `agy -px --model gemini-4-argon`).
  - `-px` and `--px` are recognized in `$launcherFlags` and automatically stripped from `$rawAgyArgs` before invoking `agy.exe`.
  - If Proxymateur is offline or unreachable when `-px` is passed, it outputs a yellow warning and falls back cleanly to native execution.

