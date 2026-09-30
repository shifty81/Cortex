# Cortex W13B2C1P — Interface-preservation and shared evidence inspection

**Dependency:** W13B2C1 must be applied first. This is a small Python-only,
transactional, overwrite-capable patch against its exact CLI preimage.

## What changes

- Adds a read-only source entry-point preservation manifest and a guard test. The
  normal FULL broad test-discovery stage runs the new test. It reports missing
  CLI/PCC entry points rather than silently accepting accidental removals.
- Adds terminal `/conversations`, `/history [ID]`, `/operations`, and
  `/operation ID`. They inspect the same existing PCC presentation-cache files
  and bounded worker traces. They **never** call a model, mutate a project,
  resume a Rust operation, or assert build/runtime success from a chat answer.
- `/status` exposes the currently selected PCC presentation-cache conversation
  ID and its limitation. `/help` documents the new commands.
- All existing terminal commands (`/status`, `/logs`, `/use`, `/exit`) and the
  natural-language path through `CortexPythonBridge` remain unchanged.

## Explicit limitation

The PCC Python conversation cache and Rust authoritative ConversationStore are
not yet one shared ID store. Browsing GUI presentation messages from CLI does
not mean pending Rust approval/execution state can be resumed by choosing that
cache ID. Do not label cross-interface conversation authority GREEN until C2B
implements and verifies that contract.

The static guard validates known entry points only. It does not replace runtime,
behavioral, portability or data-migration tests. Preserve the old Desktop backend
until useful capability parity is demonstrated; do not revive its GUI shell.

## Local run after transactional intake

```
python tools/control/CortexCapabilityGuard.py --root .
python -m unittest -v tools/control/tests/test_w13b2c1p_preservation_evidence.py
python tools/control/CortexAgentCLI.py --workspace .
```

Run PCC FULL after applying. Rust files, Vault/volume identities and existing
history are not modified by this patch.
