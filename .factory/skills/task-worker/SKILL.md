---
name: task-worker
description: >-
  Pick 2-4 unchecked tasks from TASKS.md, spawn worker subagents to implement
  them, validate with cargo test, update TASKS.md, and commit. Use when the
  user wants to make progress on the task list ("pick some tasks and do them",
  "work on TASKS.md", "grab tasks and implement").
---

# Task Worker

Execution-focused skill that picks implementable tasks from `TASKS.md`, implements
them using worker subagents, validates the result, updates the task tracker, and
commits.

This is the narrow counterpart to `launa-dev-loop`: that skill researches and
generates tasks; this one executes existing tasks.

## Phase 1: Select Tasks

1. Read `TASKS.md` from `C:\dev\launa\TASKS.md`
2. Run `git status --porcelain` to check for existing uncommitted work
3. Identify `[ ]` (unchecked) tasks that are **code-implementable**:
   - **Include**: tasks that modify workspace crates (`crates/`), tests,
     documentation, AND the ESP32 firmware (`app/`) — the ESP toolchain is
     available and `cargo +esp check` can verify `app/` code
   - **Skip**: tasks requiring physical hardware (flashing, serial testing, RS-485
     bench testing), tasks requiring physical presence ("Order USB-to-RS485
     adapter", "First field session"), tasks blocked on unchecked prerequisites
4. Prioritize by section order in TASKS.md (P0 > P1 > P2) and pick 2-4 tasks
5. Ensure selected tasks have **no file overlap** -- tasks touching the same
   source files MUST be run sequentially, not in parallel workers

### Task selection heuristic

Prefer tasks that:
- Have a clear file reference in the task description (e.g., `app/src/ota.rs`)
- Are self-contained (don't depend on other unchecked tasks)
- Can be verified with `cargo test`, `cargo check`, or `cargo +esp check`
- Are small enough for a single worker to complete in one pass

## Phase 2: Plan

1. Create a TodoWrite list with all selected tasks + validation + commit steps
2. For each task, identify the affected files from the task description and
   codebase knowledge
3. Group tasks into **parallel batches**: tasks with zero file overlap can run
   simultaneously; tasks sharing any file must be sequential

### Worker assignment rules

- **Parallel**: Tasks in different crates (e.g., one in `launa-protocol`, one in
  `launa-mqtt`), or one in a workspace crate and one in `app/` (they have
  separate build graphs — workspace uses `cargo check`, `app/` uses
  `cargo +esp check`)
- **Sequential**: Tasks that both touch the same source file, or any task that
  modifies `TASKS.md` itself
- **Max 4 workers** in a single parallel batch (resource constraint)

## Phase 3: Implement

Spawn `worker` subagents (using the `Task` tool with `subagent_type: "worker"`).

### Worker prompt template

```
You are working on the launa project at C:\dev\launa.

Your task: <description from TASKS.md, verbatim>

Files to read first:
- <list the relevant source files from the task description and references.md>

Constraints:
- Workspace crates (launa-protocol, launa-hal, launa-mqtt, launa-ota,
  launa-esp-ota, launa-sim, launa-integration-tests) must be no_std compatible.
  Use `extern crate alloc`, not `std::`.
- All protocol parsers must handle malformed input gracefully (return Result,
  never panic).
- Mock implementations behind `cfg(feature = "std")` or in test modules.
- Run `cargo test -p <crate>` after making changes to workspace crates. Fix any failures.
- If modifying `app/` (ESP32 firmware), verify with `cargo +esp check` from
  `C:\dev\launa\app`. This uses the xtensa-esp32-none-elf target via the `esp`
  toolchain. The `app/.cargo/config.toml` sets the target automatically.

When done:
1. Report what you changed (list every file path)
2. Report test results (which crate, how many tests, any failures)
3. Note any remaining issues or follow-up tasks
4. If the task cannot be completed, explain why
```

### Handling worker failures

- If a worker exits with an error, read its output to understand the failure
- If the failure is fixable (compile error, test failure), fix it directly
- If the failure is fundamental (task requires hardware, missing dependency),
  skip the task and move on
- Never re-spawn the same worker with the same prompt -- adjust the approach

## Phase 4: Validate

After all workers complete (or after each batch of parallel workers):

1. **Run `cargo test`** from `C:\dev\launa`:
   ```
   cargo test 2>&1
   ```
   All workspace tests must pass. If any fail, fix before committing.

2. **Run `cargo check`**:
   ```
   cargo check 2>&1
   ```
   No compilation errors.

3. **Run `cargo +esp check` for `app/`** (if any `app/` files were modified):
   ```
   cd C:\dev\launa\app && cargo +esp check 2>&1
   ```
   The ESP32 firmware must compile-check against `xtensa-esp32-none-elf`.
   This uses the `esp` toolchain installed via rustup. The `app/.cargo/config.toml`
   sets the target and `build-std` automatically. If this fails, fix before
   committing -- broken `app/` code blocks firmware builds and OTA deployment.

4. **Run `cargo fmt`**:
   ```
   cargo fmt
   ```
   Format all changed files.

5. If tests fail after a worker's changes, investigate and fix. Common issues:
   - Worker changed a struct field name but didn't update all references
   - Worker changed a constant (e.g., entity count) but tests assert the old value
   - Worker added a new test that is incorrect

## Phase 5: Update TASKS.md and Commit

1. **Update TASKS.md**: For each completed task, change `- [ ]` to `- [x]` and
   add a brief completion note if the task description doesn't already have one.
   Keep the existing format -- don't rewrite completed items.

2. **Review changes**: Run `git status` and `git diff` to see all changes.
   Check for:
   - No secrets, API keys, or sensitive data
   - No unintended changes to unrelated files
   - TASKS.md correctly updated

3. **Stage and commit**:
   ```
   git add <all changed files>
   git commit -m "<summary line>"
   ```

   Commit message follows AGENTS.md conventions:
   - Summary line: 50-72 chars, imperative mood
   - Body: bullet points describing key changes, be specific
   - No Co-Authored-By tags
   - One logical change per commit (or one batch of related tasks)

### Commit message examples

```
Add stale-status detection, heap monitor, graceful OTA shutdown

- Track time since last valid status frame, probe at 5s, stale at 30s
- HeapMonitor checks free heap every 60s, warns at 4 KiB, critical at 1 KiB
- On OTA trigger: publish offline, send DISCONNECT, drain UART, wait 50ms

Fix NVS partition size mismatch, add factory app partition

- Align partitions.csv NVS size with config.rs hardcoded values
- Add factory app at 0x10000 for first-flash compatibility
```

## Edge Cases

### Existing uncommitted changes
If `git status` shows uncommitted changes at the start:
- Read the diff to understand what's in progress
- Either: incorporate the changes into the current task batch, or
- Commit them separately first before starting new tasks
- Never discard uncommitted work

### Task cannot be implemented
Some tasks in TASKS.md describe future work that requires:
- Physical hardware (USB-to-RS485 adapter, spa controller)
- External services (MQTT broker, WiFi network)

Tasks requiring the ESP32 toolchain CAN be implemented -- `cargo +esp check` is
available for `app/` code verification. Skip only tasks that need actual hardware
flashing or physical presence.

### Cross-crate refactors
Some tasks touch many crates simultaneously (e.g., "refactor pumps to arrays"
touched 8+ crates). For these:
- Do NOT parallelize -- implement sequentially in dependency order:
  1. `launa-protocol` (core types)
  2. `launa-hal` (depends on protocol types)
  3. `launa-mqtt` (depends on protocol types)
  4. `launa-sim` (depends on protocol types)
  5. `launa-integration-tests` (depends on all above)
  6. `app/` (depends on all workspace crates, verified with `cargo +esp check`)
- Run `cargo test` after each workspace crate to catch issues early
- Run `cargo +esp check` after any change to `app/` or its workspace dependencies

### Worker modifies file another worker needs
This is the primary failure mode from past sessions. Prevent it by:
- Checking file overlap BEFORE spawning workers
- If overlap is unavoidable, run those workers sequentially
- If a worker reports "file changed externally", re-read the file and retry

## Loop

After committing, the skill can loop if the user wants more tasks:
- Re-read TASKS.md (now with newly checked items)
- Select the next batch of tasks
- Repeat phases 2-5

The loop stops when:
- All implementable tasks are done
- `cargo test` fails and cannot be fixed
- User interrupts
