---
name: pure-tcp-market-sniper
description: >-
  Procedures and invariants for the authoritative pure-TCP newspaper and roadside market sniper:
  120s master rotation clock, Middle Ground dual-phase streaming, -RTT/2 phase offset, True Zero-RTT
  speculative buy, 3-frame modal close sequences, and verified mutation delta checks.
---

# Pure-TCP Newspaper & Market Sniper Architecture

This skill defines the operational protocols, timing synchronizations, and packet sequences for high-speed automated purchasing of building materials (BEM/SEM/LEM) and expansion goods directly over Supercell's TCP protocol (port 9339).

---

## 1. Authoritative Tooling & Clock Synchronization

### 1.1 Master Tool Path
- **Canonical Sniper**: [`tools/protocol/tcp_newspaper_sniper.py`](file:///E:/02_Projects/Intra-farm/tools/protocol/tcp_newspaper_sniper.py).
- **Execution Command**:
  ```powershell
  python tools/protocol/tcp_newspaper_sniper.py `
      --account accounts/sniper_main.json `
      --target "Plank,Bolt,Tape,Nail,Screw,Panel" `
      --max-price 400
  ```

### 1.2 Persistent 120s Master Rotation Clock (`newspaper_clock.py`)
Supercell servers refresh the global roadside newspaper catalog on a strict **120-second epoch**:
- The master rotation state is atomically stored in `configs/newspaper_clock.json`.
- **Pre-Emption Priority & Yield Guards**:
  - Background farm jumps and long-running socket requests must check `is_rotation_imminent()`.
  - If fewer than 4 seconds remain before the rotation cycle boundary (`< 4s`), sniper workers pre-emptively yield and buffer wire traffic to prepare for catalog receipt.
- **-RTT/2 Send Phase Offset**:
  - To ensure the newspaper catalog request hits the Supercell gateway precisely at the cycle rollover, clients send pre-emptively:
    $$\text{Send Time} = \text{Target Arrival} - \frac{\text{RTT}}{2}$$

---

## 2. Sniper Packet Pipeline & Fast-Path Buying

### 2.1 Middle Ground Dual-Phase Streaming
Rather than blocking for complete packet assembly before parsing, the sniper streams TCP packets in chunks:
1. **Header Phase**: Parse 7-byte Supercell protocol header (message ID, length, client version).
2. **Speculative Stream Processing**: Decode roadside shop slot arrays incrementally as chunks arrive over the socket.

### 2.2 True Zero-RTT Speculative Buy
- If a target material (e.g. `Plank`, GID `2000002`) is identified in an incoming shop slot payload, the purchase request is dispatched **immediately** without waiting for the remainder of the shop packet to finish reading.
- Dispatches use pre-allocated, forward-folded session states from `protocol_core.pyd`.

### 2.3 3-Frame Persistent Modal Close
To ensure clean scene transitions between visited roadside stalls and return navigation:
- Dispatches a 3-frame close sequence: `14799` + `12576` + `10224`.
- Prevents server-side stuck modal states that block subsequent farm visits.

### 2.4 Chained Farm Jumps (`Msg 18671`)
- When transitioning between advertised farms, the sniper uses chained `Msg 18671` (VisitFarm) without disconnecting the gateway session.
- Avoids gateway crypto renegotiations and handshake penalties.

---

## 3. Telemetry & Verification Rules

### 3.1 Strict Mutation Verification (Rule 1.1)
- `Msg 23626` is an opaque TCP transport acknowledgment: **it does NOT confirm a successful material purchase**.
- A purchase is only confirmed when `Msg 24180` (OwnHomeData) or `Msg 24489` (VisitedHomeData) reports:
  1. An increment in the sniper account's barn storage count for the target material.
  2. A corresponding gold deduction matching the stall listing price.

### 3.2 Attempt Telemetry Logging
Every sniper attempt must record structured telemetry:
- `timestamp`: Precise millisecond wall-clock timestamp.
- `item_gid`: Target item identifier (e.g., `2000002`).
- `slot_index`: Roadside stall box index.
- `rtt_ms`: Measured round-trip latency to Supercell gateway.
- `result`: `SUCCESS`, `OUT_OF_STOCK`, `BARN_FULL`, or `RATE_LIMITED`.
