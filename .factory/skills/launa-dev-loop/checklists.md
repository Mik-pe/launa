# Launa Dev Loop -- Verification Checklist

Run this checklist at the end of each loop iteration.

## Build Verification

- [ ] `cargo test` passes from `C:\dev\launa` (all workspace tests)
- [ ] `cargo check` succeeds with no errors
- [ ] No new compiler warnings introduced

## Protocol Correctness

- [ ] Status update parser byte offsets match `docs/protocol.md`
- [ ] Command `encode()` includes correct sub-type discriminator bytes
- [ ] CRC-8 computation matches init=0x02, poly=0x07, no reflect, xorout=0x02
- [ ] Dispatcher correctly routes all `0A BF` sub-types
- [ ] Temperature encoding respects Celsius (divide by 2) vs Fahrenheit
- [ ] Pump status bit packing: pump1 bits 0-1, pump2 bits 2-3, pump3 bits 4-5

## Test Coverage

- [ ] New parsers have unit tests with valid and invalid inputs
- [ ] Integration tests use SpaSimulator for end-to-end verification
- [ ] Edge cases tested: unknown temp (0xFF), malformed frames, empty payloads
- [ ] Fuzz-like tests pass (random input resilience)

## Code Quality

- [ ] Workspace crates remain `no_std` compatible (no `std::` imports)
- [ ] All protocol parsers return `Result`, never panic on bad input
- [ ] Mock implementations behind `cfg(feature = "std")` or in test modules
- [ ] No hardcoded magic numbers -- use named constants or match arms with comments

## Task Tracking

- [ ] `TASKS.md` updated: completed items checked off, new items added
- [ ] No orphaned tasks (every task has a clear description and file reference)
- [ ] Critical bugs section is empty or has clear owners

## Loop Decision

- [ ] If unchecked tasks remain AND `cargo test` passes: continue loop
- [ ] If `cargo test` fails: fix failures, then re-verify
- [ ] If no unchecked tasks remain: loop complete, report summary to user
