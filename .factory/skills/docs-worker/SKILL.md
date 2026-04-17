---
name: docs-worker
description: Documentation worker — updates README, architecture docs, adds missing doc comments, fixes inconsistencies. Pure documentation changes, no code behavior changes.
---

# Docs Worker

NOTE: Startup and cleanup are handled by `worker-base`. This skill defines the WORK PROCEDURE.

## When to Use This Skill

Documentation-only features: updating README.md, docs/architecture.md, AGENTS.md, adding missing `///` doc comments to public API types, fixing inconsistencies (entity counts, descriptions).

## Required Skills

None.

## Work Procedure

### 1. Read Feature and Investigate

1. Read the feature description carefully
2. Read the current state of all affected documentation files
3. Read the relevant source code to understand the actual current state
4. Identify specific inconsistencies and outdated information

### 2. Update Documentation

**For architecture docs:**
1. Read the actual crate structure (Cargo.toml, lib.rs, directory listings)
2. Update docs/architecture.md to reflect current reality
3. Ensure all crates are listed with accurate descriptions
4. Update module descriptions within crates if they were refactored

**For README:**
1. Update crate table to reflect final structure
2. Fix entity counts to match actual discovery builder output
3. Update any outdated commands or descriptions
4. Keep the README concise — remove redundancy with docs/architecture.md

**For doc comments:**
1. Add `///` doc comments to public items that lack them
2. Follow existing doc comment style
3. Focus on the refactored crates first
4. Keep comments concise — explain purpose, not implementation

**For consistency fixes:**
1. Grep for entity count references across all files
2. Update all to match the actual count from `discovery.rs`
3. Update AGENTS.md if crate descriptions changed

### 3. Verify

1. `cargo doc --workspace` — docs build without errors (warning about missing docs is acceptable for pre-existing gaps)
2. Read through updated files for accuracy
3. Cross-reference with actual source code

### 4. Commit

Focused commit. Example: `Update docs/architecture.md with launa-core, launa-sim, correct HA entity count`

## Example Handoff

```json
{
  "salientSummary": "Updated docs/architecture.md to include launa-core, launa-sim, launa-esp-ota with accurate descriptions. Fixed HA entity count to 27 across README.md and AGENTS.md. Added doc comments to 12 public types in launa-protocol and launa-mqtt.",
  "whatWasImplemented": "docs/architecture.md: added launa-core (SpaApp extraction), launa-sim (simulator), launa-esp-ota (ESP32 OTA impl). Fixed entity count from 14/20 to 27 in README, AGENTS.md. Added /// doc comments to HeatingMode, TemperatureScale, TempRange, PumpState, ToggleItem, RegistrationState, TopicBuilder, DiscoveryBuilder, PumpTimer, PumpTimerManager, HoldModeTimer.",
  "whatWasLeftUndone": "",
  "verification": {
    "commandsRun": [
      { "command": "cargo doc --workspace 2>&1", "exitCode": 0, "observation": "Docs build successfully" },
      { "command": "cargo test --workspace", "exitCode": 0, "observation": "All tests still pass" }
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

- Documentation reveals a code inconsistency that should be fixed first
- Entity count in code doesn't match expectations
- Feature scope requires code changes beyond documentation
