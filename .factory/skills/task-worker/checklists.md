# Task Worker -- Verification Checklists

## Pre-Implementation Checklist

Before spawning any workers:

- [ ] Read `TASKS.md` and identified unchecked `[ ]` items
- [ ] Ran `git status --porcelain` to check for existing uncommitted work
- [ ] Selected 2-4 implementable tasks (skip hardware/blocked tasks)
- [ ] Verified no file overlap between tasks selected for parallel execution
- [ ] Created TodoWrite list tracking all tasks + validation + commit
- [ ] Checked dependency ordering for cross-crate tasks

## Per-Task Worker Checklist

Each worker should complete:

- [ ] Read all relevant source files before making changes
- [ ] Made changes to the correct files
- [ ] Added new tests for new functionality
- [ ] Updated existing tests that were affected by changes
- [ ] Ran `cargo test -p <crate>` and all tests pass (for workspace crates)
- [ ] Ran `cargo +esp check` and it succeeds (for `app/` changes)
- [ ] Reported all changed file paths
- [ ] Reported test results (count, failures)
- [ ] Reported `cargo +esp check` result (pass/fail, any errors)
- [ ] Noted any remaining issues or blockers

## Pre-Commit Checklist

Before committing:

- [ ] `cargo test` passes from workspace root (all crates)
- [ ] `cargo check` succeeds with no errors
- [ ] `cargo +esp check` succeeds from `app/` (if any `app/` files were modified)
- [ ] `cargo fmt` run on all changed files
- [ ] No new compiler warnings introduced
- [ ] TASKS.md updated: completed items changed to `[x]`
- [ ] `git diff` reviewed -- no secrets, no unintended changes
- [ ] `git status` shows expected files only
- [ ] Commit message follows AGENTS.md conventions (50-72 char summary, imperative mood)
- [ ] Commit is focused: one logical change or one batch of related tasks

## Error Recovery

If `cargo test` fails after workers complete:

- [ ] Identified which test(s) fail
- [ ] Determined if failure is from worker's changes or pre-existing
- [ ] Fixed the failure directly (don't re-spawn worker for a small fix)
- [ ] Re-ran `cargo test` to confirm fix
- [ ] If unfixable, reverted the worker's changes and skipped that task

If `cargo +esp check` fails for `app/`:

- [ ] Identified the compile error(s)
- [ ] Common causes: API mismatch with workspace crate changes, missing import,
      wrong feature flag, dependency version mismatch
- [ ] Fixed the error directly (workers modifying `app/` should verify with
      `cargo +esp check` themselves)
- [ ] Re-ran `cargo +esp check` to confirm fix
- [ ] Also ran `cargo test` to ensure workspace crates still pass

If a worker exits with error:

- [ ] Read worker output to understand failure reason
- [ ] If fixable (compile error, missing import), fixed directly
- [ ] If fundamental (needs hardware, wrong approach), skipped task
- [ ] Updated TodoWrite list to reflect skip
