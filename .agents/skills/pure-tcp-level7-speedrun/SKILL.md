---
name: pure-tcp-level7-speedrun
description: >-
  Procedures and invariants for the authoritative sub-75s pure-TCP Level 7 speedrun and high-concurrency
  fleet batch runner: 3-session balanced pacing, 2-pool cooldown laws, 0/5 gem skip mechanics, vault GIDs,
  strict Definition of Done verification, and the absolute ban on account recycling.
---

# Pure-TCP Level 7 Speedrun & Autonomous Fleet Orchestration

This skill specifies the operational protocols for minting and rapidly leveling Hay Day accounts from Level 1 to Level 7 over raw TCP sockets (port 9339) with sub-75s execution and zero emulator dependencies.

---

## 1. Speedrun Architectural Invariants

### 1.1 The Sub-75s Sweet Spot (`fast_pure_tcp_level7_speedrun.py`)
- **Execution Target**: **68–75 seconds (~1.1–1.25 min)**.
- **Session Split**: Supercell production servers physically enforce a **2-pool in-session cooldown law**. Continuous single-session leveling is rejected by the server logic engine.
- **3-Session Balanced Architecture**:
  - Total Wire Execution: ~34 seconds across 3 sessions.
  - Gateway Pacing & Session Switches: ~34 seconds.
- **Skip & Vault Invariants**:
  - Under Level 5: 0-gem instant crop skips.
  - Level 5+: 5-gem production skips.
  - Vault GIDs: Exact structure GIDs `5900081` (silo) and `5900000` (barn).

### 1.2 Deprecated & Superseded Entrypoints
- **Authoritative Tool**: [`tools/farming/fast_pure_tcp_level7_speedrun.py`](file:///E:/02_Projects/Intra-farm/tools/farming/fast_pure_tcp_level7_speedrun.py).
- **Do Not Use**: Legacy scripts that introduce crop waiting timers, multi-minute sleep loops, or deprecated single-socket sweep paths (`fast_pure_tcp_level7.py` emits a deprecation banner; `fleet_60_accounts4.py` is archived).

---

## 2. Autonomous High-Concurrency Fleet Runner (`batch_level7_fleet.py`)

For multi-account leveling operations, use the autonomous fleet runner:

### 2.1 CLI Invocations
```powershell
# 1-Shot Mint and Level (concurrency 10-15 workers)
python tools/farming/batch_level7_fleet.py --mint 20 --concurrency 12 --out-dir accounts/fleet_batch1/

# Level an Existing Directory of Raw Accounts
python tools/farming/batch_level7_fleet.py --dir accounts/raw_batch/ --concurrency 10

# Machine-Readable JSON Mode for Dashboards
python tools/farming/batch_level7_fleet.py --mint 10 --json
```

### 2.2 Operational Guardrails (Rule 0.6)
1. **Definition of Done (DoD) Gate**:
   - A batch is considered **INCOMPLETE** until an automated verification script (e.g., `deep_query_farm`) confirms 100% of accounts meet `level >= 7`.
   - If even one account remains Level 1 or desyncs, the batch task is not complete.
2. **Zero-Fallback Policy & Absolute Ban on Account Recycling**:
   - **Never borrow, copy, overwrite, or substitute pre-existing Level 7 accounts** from other directories (`accounts3/`, `accounts/test/`, `accounts/TCP-test/`) to fulfill a quota.
   - Every account delivered in a batch folder must contain its own unique Supercell credentials matching its filename.
3. **"Abandon & Replace" Definition**:
   - If an account desyncs, fails gateway authentication, or hits an unbind error, move the failed record to `.failed/`.
   - Mint a brand-new raw Supercell account via `create_supercell_farm_account()` and level it from scratch. Never mask failures.

---

## 3. Server Validation & Telemetry Verification

### 3.1 Mutation Delta Auditing
In pure-TCP operations, raw transport acknowledgments (`Msg 23626`) only confirm packet arrival at the network gateway:
- **Never claim state mutation based on transport ACKs alone**.
- Always verify inventory, XP, and gold deltas using `Msg 24180` (OwnHomeData) or `Msg 24489` (VisitedHomeData).

### 3.2 Headless Logic Engine Digital Twin (`farm_engine_sandbox.py`)
To validate wire action sequences before production execution:
```powershell
python tools/farming/farm_engine_sandbox.py --test-level7-turns
```
The sandbox is a headless offline clone of `libg.so` logic engine that models tile coordinates, crop growth timers, and vault state, validating client turns against native error codes (`0..5`).
