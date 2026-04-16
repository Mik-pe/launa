---
name: rust-app-worker
description: Implements features in the ESP32 app/ crate and cross-cutting changes (app + workspace crates, CI, gitignore, xtask). Uses TDD where possible; cargo +esp check for app/ verification.
---

# Rust App Worker

NOTE: Startup and cleanup are handled by `worker-base`. This skill defines the WORK PROCEDURE.

## When to Use This Skill

Features that involve the `app/` crate (ESP32 firmware binary), cross-cutting changes spanning app/ and workspace crates, CI pipeline setup, .gitignore changes, and xtask modifications.

## Required Skills

None.

## Work Procedure

1. **Read context**: Read the feature description, `mission.md`, `AGENTS.md`, and `.factory/library/architecture.md`. Understand what needs to change.

2. **Investigate the codebase**: Use Grep/Glob/Read to understand:
   - Exact file paths in `app/src/` that need changes
   - How app/ integrates with workspace crates
   - ESP32-specific patterns (esp-hal, embassy, no_std with alloc)
   - Existing patterns in the affected files

3. **Write tests where possible (RED)**:
   - For workspace crate changes: write failing tests first, then implement
   - For app/-only changes that can't be tested on desktop (esp-hal dependencies): write the implementation and verify with `cargo +esp check`
   - For CI/gitignore changes: verify the file exists and has correct content

4. **Implement the fix (GREEN)**:
   - Follow existing patterns in the app/ crate
   - ESP32 heap is 32 KiB — avoid unbounded allocations
   - Use `esp_println!` or logging macros for output
   - For unsafe code: always add `// SAFETY:` comments
   - For CI: create `.github/workflows/ci.yml`

5. **Verify**:
   - For workspace changes: `cargo test -p <crate>` and `cargo test --workspace`
   - For app/ changes: `cd C:\dev\launa\app && cargo +esp check`
   - Full workspace: `cargo check --workspace`
   - Format: `cargo fmt`
   - Verify no new warnings from app/: `cd C:\dev\launa\app && cargo +esp check 2>&1` — review for new warnings

6. **Update TASKS.md**: Change `- [ ]` to `- [x]` for completed tasks.

7. **Commit**: Stage and commit with descriptive message.

## Example Handoff

```json
{
  "salientSummary": "Added SAFETY comments to all UnsafeCell socket buffer dereferences in mqtt_client.rs. Added http:// scheme validation to parse_ota_url(). Both verified with cargo +esp check.",
  "whatWasImplemented": "Documented safety invariants for socket_rx_buf and socket_tx_buf UnsafeCell usage in app/src/mqtt_client.rs. Added URL scheme validation rejecting non-http:// URLs. Added warn! log on rejection.",
  "whatWasLeftUndone": "",
  "verification": {
    "commandsRun": [
      { "command": "cargo +esp check", "exitCode": 0, "observation": "app compiles with no new errors" },
      { "command": "cargo check --workspace", "exitCode": 0, "observation": "workspace typechecks" },
      { "command": "cargo fmt", "exitCode": 0, "observation": "formatted" }
    ],
    "interactiveChecks": []
  },
  "tests": {
    "added": []
  },
  "discoveredIssues": []
}
```

## When to Return to Orchestrator

- Feature depends on changes not yet made in a workspace crate
- `cargo +esp check` fails and the issue is in an esp-hal dependency (not fixable)
- Requirements need clarification about ESP32-specific behavior
- Changes needed are beyond the feature scope
