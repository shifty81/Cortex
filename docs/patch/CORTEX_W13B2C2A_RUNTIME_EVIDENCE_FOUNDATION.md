# Cortex W13B2C2A — Runtime Evidence Foundation

## Purpose

W13B2C2A closes the gap between a successful build and proof that the requested
program actually executed. It is project-agnostic infrastructure, with the first
certification target being a short-lived Rust console application.

## Changes

- Unifies runtime intent classification used by project creation and developer
  verification. Requests to execute/launch a built application, capture stdout or
  stderr, inspect an exit code, or require runtime proof set a durable runtime
  requirement. Explicit negative runtime wording remains non-runtime intent.
- Persists the runtime requirement in `.cortex/project-intent.json`, including
  completion vs. long-running mode, timeout, expected stdout, and title-change
  requirements. Approval/continue follow-ups therefore retain the original
  acceptance contract instead of losing it with the short follow-up text.
- Adds bounded short-lived project-executable capture. Cortex records the real
  process ID, executable, arguments, working directory, start/finish timestamps,
  duration, timeout state, exit code, stdout, stderr, and truncation state.
- Adds `runtime.verify_project` completion mode for console/short-lived programs.
  Nonzero exit, timeout, missing project-local executable, or an explicit stdout
  mismatch fails the runtime acceptance gate.
- Exact stdout assertions accept one normal terminal line ending (LF, CRLF, or CR)
  but reject extra lines/output.
- Preserves the existing long-running runtime/window acceptance path for GUI apps,
  including visible/responsive-window and title-change proof.
- Adds stdout capability advertising to runnable Rust/CMake profiles.
- Final verified mutation summaries can surface the runtime exit code and bounded
  captured stdout from the controller receipt rather than model prose.

## Safety / scope

- No Vault migration or storage-authority change.
- No PCC/CLI application-shell ownership change.
- No provider/model routing change.
- No arbitrary executable launch: completion evidence requires the resolved
  executable to canonicalize inside the active project root.
- Captured stdout/stderr are drained to avoid pipe deadlock and each retained
  stream is bounded to 1 MiB.
- Runtime completion timeout is bounded to 100 ms–120 s.

## First live certification

After FULL is GREEN, submit through Cortex CLI/PCC common developer workflow:

> Create a new standalone Rust console project named CortexHelloWorld. It must print exactly: Hello, world! Build it, execute the actual compiled application, capture stdout and stderr and the exit code, and do not report success without runtime proof.

The creation proposal must say `Runtime proof required: yes`. After approval,
success requires the actual built program to exit normally with code 0 and captured
stdout equal to `Hello, world!` plus at most one platform line ending.

## Certification boundary

This patch is source/fixture checked in the handoff environment, but that environment
has no Rust toolchain. PCC FULL on the user's Windows Cortex checkout remains the
authoritative Rust fmt/check/test/Clippy/build certification.
