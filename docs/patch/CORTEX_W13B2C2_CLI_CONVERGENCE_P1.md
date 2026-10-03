# W13B2C2 CLI Convergence — Pass 1

This pass is intentionally narrow and merge-safe. It preserves the local C2A
runtime-evidence implementation while correcting defects exposed by the first
CortexHelloWorld certification attempt.

## Changes

- Rust worker subprocesses receive portable Cortex/Vault authority explicitly.
- The worker environment does not force the Cortex project's `CARGO_TARGET_DIR`
  onto a managed project that Cortex may switch to during a request.
- Natural `pcc-chat` with an available Rust worker no longer starts the native
  model host before Rust has classified the request.
- `pcc-chat` emits live stage boundaries on stderr so the terminal can distinguish
  controller bootstrap from developer routing.
- Embedded PCC/CLI controller startup leaves Vault memory lazy.
- Embedded PCC/CLI startup does not archive certification conversations on every turn.
- Embedded PCC/CLI startup does not build the full Desktop presentation overview.

## Deliberately deferred

- Python provider ownership for explicit Inspect/Plan/Apply/Repair.
- Unified structured JSONL event protocol for every command.
- Complete removal of the Python presentation conversation cache.
- Conversation metadata index.
- Binary/protocol provenance handshake.
- Generated command registry/help.

Those remain part of the larger CLI-convergence milestone after Pass 1 proves
that the common controller can reach the NewProject proposal promptly.
