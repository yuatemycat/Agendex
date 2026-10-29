---
name: antigravity-experiments
description: Authoritative reference guide for Google Antigravity's experimental flags, internal binary capabilities, paired flag groups, and Proxymateur settings synchronization.
---

# Antigravity Experimental Flags & Subsystems Reference

Authoritative guide to the experimental flags discovered in `language_server.exe` and `agy.exe`, their paired functional groups, operational behaviors, and Proxymateur synchronization rules.

---

## 1. Paired Groups of Experimental Flags

The experimental flags in Antigravity are organized into architectural domains and paired groups that control language server behaviors, execution pipelines, safety gates, subagent orchestration, and UI extensions.

### A. Customizations 2.0 (Plugins, Hooks, Subagent Personas & MCP)

| CLI Flag (`settings.json`) | Proxymateur Key | Type | Description & Operational Impact |
| :--- | :--- | :--- | :--- |
| `enable-plugins` | `enable_plugins` | boolean | Enables the plugin subsystem for discovering, installing, and managing multi-asset extension bundles (`/plugin`). |
| `enable-customization-kind-agent` | `enable_customization_kind_agent` | boolean | Enables user-defined custom agent personas in `agents/` directories and dynamic registration via `define_subagent`. |
| `enable-customization-kind-skill` | `enable_customization_kind_skill` | boolean | Enables custom skill package loading and dynamic slash command dispatch (`skills/` directory). |
| `enable-customization-kind-rule` | `enable_customization_kind_rule` | boolean | Enables contextual rule injection from `rules/` directories and prompt `<RULE>` sections. |
| `enable-customization-kind-hooks` | `enable_customization_kind_hooks` | boolean | Enables lifecycle event hooks (pre-turn, post-tool, workspace-change) for deterministic intervention. |
| `enable-customization-kind-mcp` | `enable_customization_kind_mcp` | boolean | Enables Model Context Protocol (MCP) server definitions and lazy-loading via customization roots. |
| `enable-hook-status` | `enable_hook_status` | boolean | Exposes real-time status indicators, execution timings, and error telemetry for active hooks in transcripts. |
| `deprecate-workflows` | `deprecate_workflows` | boolean | Formally deprecates legacy workflow scripts in favor of Customizations 2.0 modular skills and hooks. |

### B. Sidecars (Micro-Frontends & RPC Extension System)

| CLI Flag (`settings.json`) | Proxymateur Key | Type | Description & Operational Impact |
| :--- | :--- | :--- | :--- |
| `enable-sidecars` | `enable_sidecars` | boolean | Enables hosting external sidecar extension processes via JSON-RPC. |
| `enable-ui-sidecars` | `enable_ui_sidecars` | boolean | Enables embedding interactive WebView sidecar panels alongside conversation transcripts. |
| `enable-sidecar-sdk` | `enable_sidecar_sdk` | boolean | Exposes the Antigravity Sidecar SDK API endpoints to companion processes. |
| `sidecar-gateway` | `sidecar_gateway` | boolean | Routes sidecar network and RPC requests through the local proxy gateway. |
| `sidecar-iframe-prewarm` | `sidecar_iframe_prewarm` | boolean | Pre-warms WebView iframe sandboxes in the background to achieve zero-latency UI loading. |
| `jetski-sidecar-declared-permissions` | `jetski_sidecar_declared_permissions` | boolean | Enforces strict manifest-declared permission verification before granting sidecars workspace or tool access. |

### C. DeepMind Safety & Guardrails

| CLI Flag (`settings.json`) | Proxymateur Key | Type | Description & Operational Impact |
| :--- | :--- | :--- | :--- |
| `enable-command-antivirus` | `enable_command_antivirus` | boolean | Heuristically scans proposed shell commands to block destructive patterns (e.g. recursive wipes or sandbox escapes) before execution. |
| `enable-command-assessor` | `enable_command_assessor` | boolean | DeepMind safety classifier assessing risk levels of proposed shell commands against workstation safety boundaries. |
| `allow-experimental-monitors` | `allow_experimental_monitors` | boolean | Permits experimental process and network runtime monitoring during tool execution. |
| `enable-citc-workspace-hygiene-rule` | `enable_citc_workspace_hygiene_rule` | boolean | Enforces Client-in-the-Cloud (CitC) workspace hygiene to prevent uncommitted pollution of active source trees. |
| `disable-stained-loas2-creds` | `disable_stained_loas2_creds` | boolean | Bypasses Google corporate Low Overhead Authentication Service (LOAS2) credential checks from internal builds. |

### D. Subagent Orchestration, Swarm & Messaging

| CLI Flag (`settings.json`) | Proxymateur Key | Type | Description & Operational Impact |
| :--- | :--- | :--- | :--- |
| `enable-subagent-hub` | `enable_subagent_hub` | boolean | Enables the `manage_subagents` lifecycle tool for active swarm management (`/swarm`). |
| `enable-fork-with-subagents` | `enable_fork_with_subagents` | boolean | Branches conversation trajectories and all active child subagents into isolated git worktree-style forks. |
| `enable-swarm-narrator` | `enable_swarm_narrator` | boolean | Telemetry narrator that monitors multi-agent swarms and writes real-time narrative summaries of deliberations. |
| `enable-swarm-monitor` | `enable_swarm_monitor` | boolean | Streams live status events, CPU frame budgets, and thread health for all swarm workers. |
| `send-subagent-initial-prompt-as-message` | `send_subagent_initial_prompt_as_message` | boolean | Delivers a subagent's initial prompt as an explicit reactive message rather than embedding it statically in the system prompt. |
| `enable-compact-slash-command` | `enable_compact_slash_command` | boolean | Unlocks the `/compact` slash command for active trajectory pruning, state pinning, and context window recycling. |
| `enable-remote-agent-fastpush-pin` | `enable_remote_agent_fastpush_pin` | boolean | Synchronizes agent tasks and push-pin checkpoints across remote developer machines in peer mesh networks. |
| `disable-teamwork-forced-flash-model` | `disable_teamwork_forced_flash_model` | boolean | Prevents server-side downgrade of delegated subagents to Flash; permits inheriting Pro/Ultra reasoning models. |

### E. Engine, Planning & Execution

| CLI Flag (`settings.json`) | Proxymateur Key | Type | Description & Operational Impact |
| :--- | :--- | :--- | :--- |
| `use-component-rewrite` | `use_component_rewrite` | boolean | Activates AST-aware component rewrite engine for high-precision targeted code refactoring across multiple files. |
| `use-core-direct` | `use_core_direct` | boolean | Direct IPC transport bypassing intermediary abstraction layers for core language server engine calls. |
| `use-slash-plan` | `use_slash_plan` | boolean | Enables structured interactive step-by-step planning mode via `/plan`. |
| `scheduled-tasks-v2` | `scheduled_tasks_v2` | boolean | Next-generation background task scheduler with cron syntax and wall-clock jitter controls (`schedule` tool). |
| `show-model-selection-change` | `show_model_selection_change` | boolean | Renders visible UI badges/toasts when dynamic model routing or fallback switching occurs. |
| `jetski-autonomous-mode` | `jetski_autonomous_mode` | boolean | Removes interactive confirmation gates for continuous, multi-turn autonomous execution. |
| `jetski-generative-ui` | `jetski_generative_ui` | boolean | Renders interactive HTML/JS/CSS widgets and data charts directly within conversation panes (`/generative_ui`). |
| `enable-programmatic-tools` | `enable_programmatic_tools` | boolean | Allows agents to synthesize, compile, and execute custom scripts on the fly for heavy data profiling (`/benchmark`). |
| `enable-persistent-terminals` | `enable_persistent_terminals` | boolean | Replaces one-off subshell executions with long-lived ConPTY pseudo-terminals that persist daemons and state. |
| `enable-pty` | `enable_pty` | boolean | Enables native Windows ConPTY terminal emulation for full interactive command sessions. |
| `enable-wait-tool` | `enable_wait_tool` | boolean | Equips agents with a non-blocking wait/timer tool for asynchronous polling and timers. |
| `cascade-enable-notebook-edit-tool` | `cascade_enable_notebook_edit_tool` | boolean | Allows cell-level inspection and manipulation of `.ipynb` Jupyter notebooks. |
| `cascade-use-sed-edit-tool` | `cascade_use_sed_edit_tool` | boolean | Direct streaming sed-based regex replacements for large-file editing without memory exhaustion. |
| `enable-browser-subagent-v2` | `enable_browser_subagent_v2` | boolean | Headless Chromium automation connecting directly via Chrome DevTools Protocol (CDP) for DOM/network tracing. |
| `enable-image-generator-subagent` | `enable_image_generator_subagent` | boolean | Embeds a native Google Imagen subagent to generate visual assets directly in transcripts. |

### F. Cortex & Trajectory Time-Travel

| CLI Flag (`settings.json`) | Proxymateur Key | Type | Description & Operational Impact |
| :--- | :--- | :--- | :--- |
| `cortex-snapshot-recording` | `cortex_snapshot_recording` | boolean | Continuously records workspace filesystem and conversation state snapshots. |
| `cortex-snapshot-revert` | `cortex_snapshot_revert` | boolean | Enables rolling back conversation trajectories and file states to previous snapshots. |
| `enable-conversation-search-v2` | `enable_conversation_search_v2` | boolean | Semantic trajectory vector indexing and instant search across past sessions. |

### G. Resilience, MCP & Infrastructure

| CLI Flag (`settings.json`) | Proxymateur Key | Type | Description & Operational Impact |
| :--- | :--- | :--- | :--- |
| `enable-model-capacity-exhausted-retries` | `enable_model_capacity_exhausted_retries` | boolean | Automatically retries transient Google upstream quota and model exhaustion errors (429/503). |
| `enable-mcp-non-blocking-turn-load` | `enable_mcp_non_blocking_turn_load` | boolean | Loads Model Context Protocol (MCP) tool schemas asynchronously without blocking UI turns. |
| `enable-voice-activity-events` | `enable_voice_activity_events` | boolean | Activates built-in voice event triggers and audio streamer daemon support (`agy mic-serve`). |
| `enable-ripgrep-path-search` | `enable_ripgrep_path_search` | boolean | Accelerates file path and symbol lookups using an integrated high-performance ripgrep engine. |
| `enable-owl-slash-command` | `enable_owl_slash_command` | boolean | Unlocks `/owl` deep synthesis pipeline (DeepInvestigator + DeepCoder). |
| `enable-deepagent` | `enable_deepagent` | boolean | Deep multi-pass reasoning loop and autonomous verification pipeline. |
| `enable-battle-mode` | `enable_battle_mode` | boolean | Adversarial agent arena pitting competing models against each other (`/battle`). |
| `enable-battle-mode-custom-agents` | `enable_battle_mode_custom_agents` | boolean | Enables pitting user-defined custom agent personas against each other in the Colosseum arena. |
| `enable-review` | `enable_review` | boolean | Activates structured code review checklist and auditing protocol. |

---

## 2. Dynamic Slash Command Architecture

In `agy.exe`, slash commands are not static binary macros. Except for four core builtins (`/clear`, `/permissions`, `/usage`, `/workspace`), all slash commands are dynamically loaded from `~/.gemini/config/skills/<name>/SKILL.md`:

- **`/owl <task>`** &rarr; Architectural synthesis via `owl` skill.
- **`/battle <task>`** &rarr; Colosseum adversarial duel via `battle` skill.
- **`/swarm <task>`** &rarr; Hands-free autonomous swarm via `swarm` skill.
- **`/benchmark <task>`** &rarr; Programmatic tool synthesis & profiling via `benchmark` skill.
- **`/generative_ui <task>`** &rarr; Interactive widget generation via `generative_ui` skill.
- **`/compact`** &rarr; Context compaction and trajectory pruning (`enable-compact-slash-command`).
- **`/plan <task>`** &rarr; Step-by-step interactive engineering planning (`use-slash-plan`).
- **`/plugin`** &rarr; Customizations 2.0 plugin management and bundling (`enable-plugins`).

---

## 3. New CLI Capabilities

### A. Subagent Reactive Initial Prompting
When `send-subagent-initial-prompt-as-message` is enabled, the parent agent sends the subagent's task description via the asynchronous messaging channel (`send_message`) rather than injecting it statically into the child agent's initial system prompt. This ensures clean separation between the child agent's persona identity and its dynamic task payload, enabling reactive wakeups and multi-turn subagent workflows.

### B. Scheduled Tasks v2 with Cron Syntax
The `scheduled-tasks-v2` capability upgrades the agent's scheduling subsystem. Rather than simple duration-based timers, it supports standard 5-field cron expressions (e.g. `*/15 * * * *`), wall-clock jitter prevention via `ExactTiming`, and conditional early cancellations via `TimerCondition`.

### C. AST Component Rewrite Engine
With `use-component-rewrite`, the language server utilizes AST-guided component boundary analysis when performing refactors, preventing syntax regressions and incomplete block replacements across large component hierarchies.

### D. Direct Core IPC Transport
With `use-core-direct`, command execution and streaming telemetry bypass intermediary shim layers, communicating directly with the core daemon via named pipes or local sockets for lower latency and reduced IPC serialization overhead.

---

## 4. Configuration Locations & Synchronization

### Storage Locations
- **CLI Configuration**: `~/.gemini/antigravity-cli/settings.json` under the `"experiments"` object.
- **Proxymateur Configuration**: `~/.proxymateur/gui_config.json` under the `"antigravity_lab"` key.

### CLI Prefix Recognition Pattern
The `Sync-CliExperimentFlags` function in `scripts/agy_gateway.ps1` and `E:\03_Devops\AntigravityManager\bin\agy_gateway.ps1` automatically parses experimental flags passed on the CLI and synchronizes them into `settings.json`:

```regex
^--(enable|disable|jetski|cascade|cortex|sidecar|use|scheduled|show|deprecate|allow)-[a-z0-9-]+(=(true|false))?$
```

Passing `--enable-plugins=true` or `--disable-teamwork-forced-flash-model` on launch automatically persists the configuration to `~/.gemini/antigravity-cli/settings.json`.
