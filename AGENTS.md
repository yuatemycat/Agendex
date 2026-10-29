# Agent Guidelines & Routing Hub (`AGENTS.md`)

> **Tier 1 Context Router (Budget: <150 lines)** — Load only the Tier 2 Skill or Tier 3 Spec relevant to your current task. Never dump or cite `AGENTS.md` rule numbers in runtime tool stdout/stderr.

---

## 1. Project Mission & Decoupled Subsystem Topology

**Agendex** is an open-source, local-first autonomous agent harness, execution supervisor, and trajectory state engine in Rust. It operates across three decoupled, synchronized layers:
1. **Execution Subsystem (`agendex-harness`, `agendex-ast`, `agendex-mcp`)**: ConPTY persistent terminal supervisor ([`crates/agendex-harness/src/terminal.rs`](file:///E:/02_Projects/Agendex/crates/agendex-harness/src/terminal.rs)), background task ring-buffer supervisor ([`crates/agendex-harness/src/tasks.rs`](file:///E:/02_Projects/Agendex/crates/agendex-harness/src/tasks.rs)), native file/command tools ([`crates/agendex-harness/src/tools.rs`](file:///E:/02_Projects/Agendex/crates/agendex-harness/src/tools.rs)), AST syntax validation & streaming patcher ([`crates/agendex-ast/`](file:///E:/02_Projects/Agendex/crates/agendex-ast/)), and stdio JSON-RPC Model Context Protocol host ([`crates/agendex-mcp/src/lib.rs`](file:///E:/02_Projects/Agendex/crates/agendex-mcp/src/lib.rs)).
2. **Trajectory & State Subsystem (`agendex-trajectory`)**: Dual-tier JSONL logging ([`crates/agendex-trajectory/src/trajectory.rs`](file:///E:/02_Projects/Agendex/crates/agendex-trajectory/src/trajectory.rs); `trajectory.jsonl` compact / `trajectory_full.jsonl` lossless audit), atomic Git `write-tree` snapshots & rollbacks ([`crates/agendex-trajectory/src/snapshots.rs`](file:///E:/02_Projects/Agendex/crates/agendex-trajectory/src/snapshots.rs)), and Head-Tail Compactor with post-compaction immunity ([`crates/agendex-trajectory/src/compactor.rs`](file:///E:/02_Projects/Agendex/crates/agendex-trajectory/src/compactor.rs)).
3. **Scaffolding & Multi-Protocol Model Core (`agendex-scaffold`, `agendex-core`, `agendex-cli`)**: Folder-based `SKILL.md` parser ([`crates/agendex-scaffold/src/skills.rs`](file:///E:/02_Projects/Agendex/crates/agendex-scaffold/src/skills.rs)), scoped rule hierarchies ([`crates/agendex-scaffold/src/rules.rs`](file:///E:/02_Projects/Agendex/crates/agendex-scaffold/src/rules.rs)), deterministic lifecycle hooks ([`crates/agendex-scaffold/src/hooks.rs`](file:///E:/02_Projects/Agendex/crates/agendex-scaffold/src/hooks.rs)), ephemeral tool synthesis ([`crates/agendex-scaffold/src/ephemeral.rs`](file:///E:/02_Projects/Agendex/crates/agendex-scaffold/src/ephemeral.rs)), reactive mailbox ([`crates/agendex-core/src/mailbox.rs`](file:///E:/02_Projects/Agendex/crates/agendex-core/src/mailbox.rs)), and unified `ModelClient` wire engine ([`crates/agendex-core/src/model.rs`](file:///E:/02_Projects/Agendex/crates/agendex-core/src/model.rs)) translating Google Gemini, OpenAI, and Anthropic Claude formats.

---

## 2. Top 5 Non-Negotiable Operational Rules

1. **Authoritative Verification, Mutation Laws & DoD (Never Trust Unverified LLM Output)**:
   - **Test & Format Contract**: Every change must compile and pass workspace tests cleanly: `& "E:\03_Devops\env.ps1"; cargo test --workspace` (all 88+ tests green) and `cargo fmt --all -- --check`.
   - **Zero-Unverified-Theories Law**: Never claim a feature or fix works without executing end-to-end test verification yourself. Do not ask the user to verify what tests can assert.
   - **Syntax Gate & Atomic Snapshotting**: Mutating file edits must use exact block or parsed AST matching; reject unparseable syntax (`SyntaxGate`). Before modifying workspace files, record a Git tree hash (`git write-tree`); verify rollback validity via `SnapshotEngine::rollback_latest`.
   - **Deterministic Tool Results**: Hook, tool, or subagent failures must return structured errors (`ToolResult::error`) into the model loop; never panic, abort the process, or mask failures as conversational text (`Ok("Model execution notice...")`).
2. **Multi-Protocol Wire Parity & Zero Leaks (Gemini, Claude, OpenAI)**:
   - Always validate `ModelClient` request and response encoding across all three supported formats:
     - **Gemini**: System prompts sent via top-level `systemInstruction`; tool dispatches mapped strictly to `functionCall` / `functionResponse` blocks (never plain text pseudo-calls); non-assistant turns strictly typed.
     - **Anthropic**: Native `tool_use` mapped directly to `tool_result` without echoing duplicate plain text; preserve thinking tokens and cache control headers.
     - **OpenAI**: Strict schema `tools` array with typed `tool_calls` parameter resolution.
   - HTTP transport or gateway errors must return `Err(AgendexError::ModelError(...))` immediately to trigger caller retries or error escalation.
3. **Execution Supervisor & Terminal Lifecycle Hygiene**:
   - **Bounded ConPTY Lifecycles**: Terminal sessions and commands exceeding `wait_ms` transition deterministically into managed background task IDs (`agendex-harness/src/tasks.rs`). Child processes on Windows must bind to Job Objects to guarantee clean teardown on exit.
   - **Task Log & Context Window Hygiene (Preventing Auto-Compaction Amnesia)**:
     - **NEVER** dump or read whole `.system_generated/tasks/*.log` files or unbounded ConPTY terminal rings into agent context without bounds. Use `tail_task_log` or bounded range reads (last 30–50 lines).
     - **Binary File Read Prohibition**: NEVER call `read_file` on binary image, document, archive, or executable files (`.png`, `.exe`, `.dll`, `.so`, `.bin`). Inspect via lightweight headless CLI probes.
4. **Trajectory State, Head-Tail Compactor & Reactive Wakeup**:
   - **Dual-Tier State**: Active model turns feed from lightweight `trajectory.jsonl`; full lossless auditing streams to `trajectory_full.jsonl`. Truncate both tiers on `/revert`.
   - **Head-Tail Compactor Law**: Compaction triggers when cumulative turn tokens breach model ceilings (32k/64k/80k/128k/200k/256k/840k/1M/2M). Always retain initial user goals (Head) and active execution state (Tail), synthesize intermediate steps into structured anchors (`Task Overview`, `Key Decisions`, `Next Steps`), and set the single-session immunity token so compaction never recurses.
   - **Reactive Wakeup (Zero Sleep-Polling)**: Wait on queues or `tokio::select!` for subagent messages and task completion. Never write busy-wait loops (`std::thread::sleep` or polling loops).
5. **Single Source of Truth, Crate Modularity & Toolchain Discipline**:
   - **Toolchain Invariant**: Always build using the portable toolchain at `E:\03_Devops` (`& "E:\03_Devops\env.ps1"`; Cargo 1.99+, MSVC `cl.exe`, rustls). Never install dev tools to `C:\Users\<user>\AppData` or default user paths.
   - **Crate Modularity**: Edit live code only within canonical crate boundaries (`crates/agendex-*`). No cross-crate circular dependencies. Maintain public API contracts in `agendex-core::AgendexAgent`.
   - **Anti-Sprawl & Clean Commits**: Temporary probes belong in `scratch/`. Prune superseded experiments before committing. Code >10 lines must be in concrete files with clickable markdown links (`file:///...`).

---

## 3. Repository Directory & Canonical Crate Switchboard

| Directory / Crate | Role & Edit Policy | Key Canonical Entrypoints |
| :--- | :--- | :--- |
| `crates/agendex-core/` | **Agent Orchestrator, Swarm & Wire Client** | [`agent.rs`](file:///E:/02_Projects/Agendex/crates/agendex-core/src/agent.rs) (`AgendexAgent`, `process_turn`), [`model.rs`](file:///E:/02_Projects/Agendex/crates/agendex-core/src/model.rs) (`ModelClient`), [`mailbox.rs`](file:///E:/02_Projects/Agendex/crates/agendex-core/src/mailbox.rs), [`swarm.rs`](file:///E:/02_Projects/Agendex/crates/agendex-core/src/swarm.rs), [`battle.rs`](file:///E:/02_Projects/Agendex/crates/agendex-core/src/battle.rs), [`safety.rs`](file:///E:/02_Projects/Agendex/crates/agendex-core/src/safety.rs), [`worktree.rs`](file:///E:/02_Projects/Agendex/crates/agendex-core/src/worktree.rs) |
| `crates/agendex-harness/` | **Process, ConPTY & Native Tools** | [`terminal.rs`](file:///E:/02_Projects/Agendex/crates/agendex-harness/src/terminal.rs) (`TerminalSupervisor`), [`tasks.rs`](file:///E:/02_Projects/Agendex/crates/agendex-harness/src/tasks.rs) (`TaskManager`), [`tools.rs`](file:///E:/02_Projects/Agendex/crates/agendex-harness/src/tools.rs) (`ToolRegistry`, file/cmd/task tools) |
| `crates/agendex-trajectory/` | **Trajectory Logs, Compaction & Snapshots** | [`trajectory.rs`](file:///E:/02_Projects/Agendex/crates/agendex-trajectory/src/trajectory.rs) (`TrajectoryLogger`), [`snapshots.rs`](file:///E:/02_Projects/Agendex/crates/agendex-trajectory/src/snapshots.rs) (`SnapshotEngine`), [`compactor.rs`](file:///E:/02_Projects/Agendex/crates/agendex-trajectory/src/compactor.rs) (`ContextCompactor`) |
| `crates/agendex-ast/` | **AST Validation & File Patching** | [`patcher.rs`](file:///E:/02_Projects/Agendex/crates/agendex-ast/src/patcher.rs) (`exact_replace`, `streaming_regex_replace`), [`ast.rs`](file:///E:/02_Projects/Agendex/crates/agendex-ast/src/ast.rs) (`TreeSitterParser`), [`validator.rs`](file:///E:/02_Projects/Agendex/crates/agendex-ast/src/validator.rs) (`SyntaxGate`) |
| `crates/agendex-scaffold/` | **Skills, Rules, Hooks & Ephemeral Tools** | [`skills.rs`](file:///E:/02_Projects/Agendex/crates/agendex-scaffold/src/skills.rs) (`SkillRegistry`), [`rules.rs`](file:///E:/02_Projects/Agendex/crates/agendex-scaffold/src/rules.rs) (`RuleEngine`), [`hooks.rs`](file:///E:/02_Projects/Agendex/crates/agendex-scaffold/src/hooks.rs) (`HookDispatcher`), [`ephemeral.rs`](file:///E:/02_Projects/Agendex/crates/agendex-scaffold/src/ephemeral.rs) (`EphemeralToolManager`) |
| `crates/agendex-mcp/` | **Model Context Protocol Host** | [`lib.rs`](file:///E:/02_Projects/Agendex/crates/agendex-mcp/src/lib.rs) (`McpClient`, stdio JSON-RPC transport, lazy schema loading) |
| `crates/agendex-cli/` | **Interactive REPL & CLI Binaries** | [`main.rs`](file:///E:/02_Projects/Agendex/crates/agendex-cli/src/main.rs), [`lib.rs`](file:///E:/02_Projects/Agendex/crates/agendex-cli/src/lib.rs) (CLI argument parser, REPL, headless batch runner) |
| `.agenda/` & `.agents/` | **Active Scaffolding Roots** | `.agenda/skills/`, `.agenda/rules/`, `.agents/skills/` (drop-in markdown skills & rules) |
| `scripts/` | **Portable Build & Automation Scripts** | [`build_release.ps1`](file:///E:/02_Projects/Agendex/scripts/build_release.ps1) (release binary compiler) |

---

## 4. Progressive Disclosure Router (Tier 2 Skills & Tier 3 Deep Specs)

Start with **`code-review-graph` MCP** (`list_graph_stats_tool`, `semantic_search_nodes_tool`, `query_graph_tool`, `get_review_context_tool`) before scanning files. Then load **only** the matching domain guide:

| Task Domain | Tier 2 Skill (`.agents/skills/<name>/SKILL.md`) | Tier 2 / Tier 3 Deep Spec & Plan |
| :--- | :--- | :--- |
| **Model Loop & Wire Protocols** | [`antigravity-experiments`](file:///E:/02_Projects/Agendex/.agents/skills/antigravity-experiments/SKILL.md), [`windows-devops-toolchains`](file:///E:/02_Projects/Agendex/.agents/skills/windows-devops-toolchains/SKILL.md) | [`PLAN.md`](file:///E:/02_Projects/Agendex/PLAN.md) § Phase 5, [`README.md`](file:///E:/02_Projects/Agendex/README.md) § 2.A |
| **Trajectory & Context Compaction** | [`compact`](file:///E:/02_Projects/Agendex/.agents/skills/compact/SKILL.md), [`compaction`](file:///E:/02_Projects/Agendex/.agents/skills/compaction/SKILL.md) | [`PLAN.md`](file:///E:/02_Projects/Agendex/PLAN.md) § Phase 2, [`crates/agendex-trajectory/src/compactor.rs`](file:///E:/02_Projects/Agendex/crates/agendex-trajectory/src/compactor.rs) |
| **AST Parsing & Code Patching** | [`superreviewer`](file:///E:/02_Projects/Agendex/.agents/skills/superreviewer/SKILL.md) | [`PLAN.md`](file:///E:/02_Projects/Agendex/PLAN.md) § Phase 3, [`crates/agendex-ast/src/patcher.rs`](file:///E:/02_Projects/Agendex/crates/agendex-ast/src/patcher.rs) |
| **ConPTY Terminal & Harness** | [`windows-devops-toolchains`](file:///E:/02_Projects/Agendex/.agents/skills/windows-devops-toolchains/SKILL.md), [`windows-portable-build-tools`](file:///E:/02_Projects/Agendex/.agents/skills/windows-portable-build-tools/SKILL.md) | [`PLAN.md`](file:///E:/02_Projects/Agendex/PLAN.md) § Phase 1, [`crates/agendex-harness/src/terminal.rs`](file:///E:/02_Projects/Agendex/crates/agendex-harness/src/terminal.rs) |
| **Skills, Rules & Lifecycle Hooks** | [`agy-customizations`](file:///C:/Users/X/.gemini/antigravity-cli/builtin/skills/agy-customizations/SKILL.md), [`plugin`](file:///E:/02_Projects/Agendex/.agents/skills/plugin/SKILL.md) | [`PLAN.md`](file:///E:/02_Projects/Agendex/PLAN.md) § Phase 4, [`crates/agendex-scaffold/src/hooks.rs`](file:///E:/02_Projects/Agendex/crates/agendex-scaffold/src/hooks.rs) |
| **Model Context Protocol (MCP)** | [`homelab-hub-routing`](file:///E:/02_Projects/Agendex/.agents/skills/homelab-hub-routing/SKILL.md) | [`PLAN.md`](file:///E:/02_Projects/Agendex/PLAN.md) § Phase 6, [`crates/agendex-mcp/src/lib.rs`](file:///E:/02_Projects/Agendex/crates/agendex-mcp/src/lib.rs) |
| **Autonomous Swarms & Battle Arena** | [`swarm`](file:///E:/02_Projects/Agendex/.agents/skills/swarm/SKILL.md), [`battle`](file:///E:/02_Projects/Agendex/.agents/skills/battle/SKILL.md), [`owl`](file:///E:/02_Projects/Agendex/.agents/skills/owl/SKILL.md) | [`PLAN.md`](file:///E:/02_Projects/Agendex/PLAN.md) § Phase 9, [`crates/agendex-core/src/swarm.rs`](file:///E:/02_Projects/Agendex/crates/agendex-core/src/swarm.rs) |
| **Performance Benchmarks** | [`benchmark`](file:///E:/02_Projects/Agendex/.agents/skills/benchmark/SKILL.md), [`web-perf`](file:///E:/02_Projects/Agendex/.agents/skills/web-perf/SKILL.md) | [`README.md`](file:///E:/02_Projects/Agendex/README.md) § 2.D, [`crates/agendex-scaffold/src/ephemeral.rs`](file:///E:/02_Projects/Agendex/crates/agendex-scaffold/src/ephemeral.rs) |
| **Queue & Continuation Control** | [`queue`](file:///E:/02_Projects/Agendex/.agents/skills/queue/SKILL.md), [`con`](file:///E:/02_Projects/Agendex/.agents/skills/con/SKILL.md) | [`PLAN.md`](file:///E:/02_Projects/Agendex/PLAN.md) § 1, [`crates/agendex-core/src/mailbox.rs`](file:///E:/02_Projects/Agendex/crates/agendex-core/src/mailbox.rs) |
| **Release & Phased Roadmap** | [`superreviewer`](file:///E:/02_Projects/Agendex/.agents/skills/superreviewer/SKILL.md) | [`PLAN.md`](file:///E:/02_Projects/Agendex/PLAN.md) § Phase 10, [`scripts/build_release.ps1`](file:///E:/02_Projects/Agendex/scripts/build_release.ps1) |

---

## 5. Key Live Findings & Runtime Contracts (Live-Verified on Windows 11)

- **MSVC Build Toolchain Requirement**: Cargo crates linking C dependencies (`ring`, `libsqlite3-sys`) require MSVC build tools (`cl.exe`). Always prefix build and test invocations with `& "E:\03_Devops\env.ps1"` in PowerShell sessions.
- **Git Write-Tree Snapshots**: Snapshot capture uses `git write-tree` to generate atomic tree SHAs without mutating the Git index or working tree; `/revert` restores via `git read-tree` and resets workspace files cleanly.
- **Compaction Post-Immunity Invariant**: Context compaction writes a structured anchor (`Task Overview`, `Key Decisions`, `Next Steps`) and injects a single-session immunity token. Compaction cannot trigger consecutively in the same session, preventing compaction recursion.
- **ConPTY Windows Job Objects**: On Windows 11, ConPTY child processes (`agendex-harness`) are bound to OS Job Objects with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` so abrupt agent termination leaves zero orphaned background shells.
- **Syntax Gate Fallback**: When an AST edit breaks parse trees, the patcher rolls back in memory, preserving the original file intact and returning a structured `SyntaxError` tool result instead of corrupting source code.
- **Test Suite Contract**: Workspace test baseline: 88 passing tests across 7 crates (`cargo test --workspace`). All phases must maintain 100% green tests before committing.
