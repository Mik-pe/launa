---
name: launa-dev-loop
description: >-
  Autonomous research-and-implement loop for the launa project. Spawns worker
  subagents to research docs and online references, cross-references findings
  against the current code, creates tasks in TASKS.md or docs in docs/, then
  spawns other workers to implement the tasks. Loops until the project is
  essentially done. Use when the user wants to run an autonomous development
  cycle on launa.
---

# Launa Dev Loop

Autonomous development loop that researches, plans, and implements work on the
launa ESP32 Balboa spa controller firmware project.

## When to Use

- User wants to run an autonomous development cycle ("loop the following",
  "keep working on the project", "run the dev loop")
- User wants to identify and fix gaps between docs and code
- User wants to push the project forward end-to-end without manual guidance

## Loop Overview

```
┌─────────────┐    ┌──────────────┐    ┌──────────────┐    ┌──────────────┐
│   RESEARCH   │───>│ CROSS-REF   │───>│  TASK GEN   │───>│  IMPLEMENT  │
│  (workers)   │    │  (workers)  │    │ (TASKS.md)  │    │  (workers)  │
└─────────────┘    └──────────────┘    └──────────────┘    └──────────────┘
       ^                                                          │
       └────────────── loop if tasks remain ─────────────────────┘
```

## Phase 1: Research (Spawn Workers)

Spawn 2-4 worker subagents in parallel to gather knowledge:

### Worker A: Protocol References
- Read `docs/protocol.md` and `docs/bp6013g1.md` thoroughly
- Search online for the Balboa Worldwide App protocol reference:
  `https://github.com/ccutrer/balboa_worldwide_app/blob/main/doc/protocol.md`
- Search for reference implementations:
  - `https://github.com/cribskip/esp8266_spa` (Arduino)
  - `https://github.com/jasta/esp32-balboa-spa` (Rust)
- Extract: message types, byte offsets, CRC details, registration flow,
  toggle codes, temperature encoding, fault codes, filter cycle format

### Worker B: Home Assistant MQTT Patterns
- Read `crates/launa-mqtt/src/discovery.rs` and `crates/launa-mqtt/src/state.rs`
- Search for Home Assistant MQTT auto-discovery documentation
- Extract: topic patterns, discovery payload schemas, component types,
  availability topics, command/state topic conventions

### Worker C: ESP32 Embedded Rust Patterns
- Read `app/src/main.rs` and `app/Cargo.toml`
- Search for `esp-idf-svc` and `esp-idf-hal` examples and documentation
- Extract: UART setup, WiFi config, MQTT client usage, OTA update patterns,
  NVS storage, GPIO direction pin control for RS-485

### Worker D: Current Codebase State
- Read ALL source files in `crates/` and `app/`
- Read `TASKS.md` to understand what's done and what's pending
- Read `docs/architecture.md` for the crate structure
- Extract: what's implemented, what's missing, what has bugs

**Worker prompt template:**

```
You are researching the launa project at C:\dev\launa.

<specific instructions for this worker>

Report your findings as a structured list:
1. Key facts discovered
2. Gaps or inconsistencies found
3. Specific file:line references where changes may be needed
4. URLs or references that were particularly useful
```

## Phase 2: Cross-Reference (Spawn Workers)

Spawn 2-3 worker subagents to compare research findings against the code:

### Cross-Ref Worker 1: Protocol vs Code
Compare protocol docs against implementation:
- `docs/protocol.md` byte offsets vs `crates/launa-protocol/src/status.rs`
- Command sub-type bytes in `crates/launa-protocol/src/command.rs` vs protocol spec
- Dispatcher sub-type handling in `crates/launa-protocol/src/dispatcher.rs`
- CRC-8 implementation in `crates/launa-protocol/src/crc8.rs`
- Registration state machine in `crates/launa-protocol/src/registration.rs`

Report each discrepancy as: `[SEVERITY] file:line -- expected X, found Y`

### Cross-Ref Worker 2: MQTT vs HA Requirements
Compare MQTT implementation against HA auto-discovery requirements:
- `crates/launa-mqtt/src/discovery.rs` entity configs
- `crates/launa-mqtt/src/state.rs` JSON serialization
- `crates/launa-mqtt/src/command_parser.rs` command handling
- `crates/launa-mqtt/src/topics.rs` topic naming

### Cross-Ref Worker 3: Test Coverage Gaps
Analyze test coverage:
- Which modules have tests? Which don't?
- Are there integration tests in `crates/launa-integration-tests/`?
- Does the spa simulator in `crates/launa-integration-tests/src/spa_simulator.rs`
  match the real protocol?
- Are edge cases tested (malformed frames, unknown temps, etc.)?

## Phase 3: Task Generation

Based on cross-reference findings, update the project's task list:

1. Read current `TASKS.md`
2. Add newly discovered tasks under the appropriate sections:
   - **Critical Bugs** -- for wrong byte offsets, missing protocol bytes
   - **Protocol Parser** -- for missing message parsers
   - **MQTT / Home Assistant** -- for MQTT gaps
   - **HAL / Desktop Testing** -- for test gaps
   - **ESP32 Firmware** -- for firmware gaps
3. Mark any tasks that were already completed
4. Create new docs in `docs/` if significant new knowledge was discovered

### Task format (follow existing TASKS.md conventions):

```markdown
- [ ] **Brief description** (`path/to/file.rs`): Detailed explanation of what needs
  to change and why, with reference to protocol doc or external source.
```

## Phase 4: Implementation (Spawn Workers)

Spawn workers to implement tasks from TASKS.md. Prioritize:

1. **Critical bugs first** (wrong byte offsets, missing sub-type bytes)
2. **Protocol parsers** (information, fault log, filter cycles responses)
3. **Tests** (integration tests, edge case coverage)
4. **MQTT** (state serialization, command parsing)
5. **ESP32 firmware** (UART transport, WiFi, MQTT client)

### Worker prompt template for implementation:

```
You are working on the launa project at C:\dev\launa.

Your task: <description from TASKS.md>

Files to read first:
- <list relevant source files>

Constraints:
- Workspace crates (launa-protocol, launa-hal, launa-mqtt, launa-ota) must be
  no_std compatible. Use `extern crate alloc`, not `std::`.
- The `app/` crate uses esp-idf-svc/hal and has `std` available.
- All protocol parsers must handle malformed input gracefully (return Result,
  never panic).
- Run `cargo test` after making changes. Fix any failures.

When done:
1. Report what you changed
2. Report test results
3. Note any remaining issues or follow-up tasks
```

### Parallelism guidelines:
- Spawn up to 4 workers in parallel for independent tasks
- Workers modifying the same file MUST be sequential
- Group related tasks for the same worker (e.g., all protocol parser fixes)

## Phase 5: Verify and Loop

After implementation workers complete:

1. Run `cargo test` from `C:\dev\launa` -- all tests must pass
2. Run `cargo check` to verify no compilation errors
3. Read the updated `TASKS.md`
4. If unchecked tasks remain and `cargo test` passes, loop back to Phase 1
5. If `cargo test` fails, fix failures before looping

### Termination conditions:
- All tasks in `TASKS.md` are checked off
- `cargo test` passes with zero failures
- No new gaps discovered in the last research cycle
- OR user interrupts the loop

## Key Constraints

- **Canonical protocol reference**: `docs/protocol.md` takes precedence over
  external references. If external sources contradict the local docs, flag it
  as a task to resolve the discrepancy.
- **no_std**: All workspace crates must compile without `std`. The `alloc`
  crate is available. Only the `app/` crate has full `std`.
- **Build toolchain**: Workspace crates use standard `cargo test`. The `app/`
  crate requires ESP-IDF toolchain and `cargo espflash` -- don't try to
  build/flash it in the loop unless the user explicitly asks.
- **Worker droid**: Use the `worker` custom droid (defined in personal droids)
  for all subagent tasks. It inherits the model and has file editing tools.
- **Don't duplicate work**: Before spawning workers, read `TASKS.md` to see
  what's already done. Don't re-implement completed tasks.

## Project File Map

```
C:\dev\launa\
├── Cargo.toml                    # Workspace root (excludes app/)
├── TASKS.md                      # Task tracker -- the loop's source of truth
├── docs/
│   ├── architecture.md           # Crate structure and data flow
│   ├── protocol.md               # Balboa protocol reference (canonical)
│   └── bp6013g1.md               # BP6013G1 controller hardware notes
├── crates/
│   ├── launa-protocol/           # Protocol parser (no_std)
│   ├── launa-hal/                # Hardware abstraction traits + mocks
│   ├── launa-mqtt/               # MQTT + HA discovery (feature-gated std)
│   ├── launa-ota/                # OTA update trait + mock
│   └── launa-integration-tests/  # Integration tests with SpaSimulator
└── app/                          # ESP32 firmware (excluded from workspace)
    └── src/main.rs               # Main firmware entry point
```
