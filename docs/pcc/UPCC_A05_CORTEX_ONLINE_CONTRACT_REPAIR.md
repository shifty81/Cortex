# Cortex A05 — desktop contract migration, bounded single-source change

Source reference: published Cortex `5579ecdc1bbb2fa0935315bec7b838d496752f8b`, A04 payload exact checksum. A05 targets A04-installed file preimages only; do not force on dirty, newer or unverified source.

## Exact problem

Published `project.control.json` declares `fmt.apply.rollback=git_or_snapshot` and `forge.rust.run.rollback=process_stop`; `cortex_project::RollbackPolicy` accepts `none`, `snapshot`, `transactional`, `provider_owned`. The earlier A02/A04 read-only auditor properly blocks Desktop before its native process starts. A05 does NOT weaken the auditor or add a deserialization alias that falsely promises rollback semantics.

## What this pass changes

- Provides a Cortex-only `contract-migrate` command; default PREVIEW is read-only and does not construct CortexPCC or execute any commands.
- Explicit `--yes` changes only two JSON string tokens when exact project identity, schema, command keys, executable, args, risk, side effects, permissions and bounded process cancellation match the audited source.
- Both legacy `rollback` values become `none`: `git_or_snapshot` had no verified automatic formatter snapshot implementation; `process_stop` is cancellation, represented independently by unchanged `bounded_kill`, not rollback. This is **truthful metadata**, not a new recovery implementation. `fmt.apply` retains local_mutation/workspace_write; it must still be authorized and backed up separately before use. The migration does not execute formatting or Forge.
- Creates and verifies byte-exact backup under `artifacts/recovery/contract/<unique-id>/project.control.json`, checksum-checks the file immediately before replacement, uses atomic replacement and writes a receipt. Uses existing PCC operation lock. If a post-apply check fails, the original backup is retained and must be recovered deliberately; never infer rollback succeeded.
- Registers the nine A05 tests in the existing mandatory Python PCC Full Gate stage.
- Enhances Desktop launch failure messages to point to the preview and explicit repair; no preflight auto-rewrite or false UI-ready claim.

## Operator path (only if A04 installed and patch preimages match)

1. Apply A05 using the normal root-drop transactional patch authority and restart the controller if requested. A05 itself does not modify `project.control.json`.
2. `python tools/control/CortexPCC.py contract-migrate --root .` — read preview, especially the explicit **no automatic rollback** warning.
3. If the exact two migration rows match the actual local source and you approve them, run `python tools/control/CortexPCC.py contract-migrate --root . --yes`.
4. `python tools/control/CortexPCC.py universal-plan --root . --rust-consumer` and `python tools/control/CortexPCC.py full --root .`; only after a fresh Full Gate launch `python tools/control/CortexPCC.py launch-gui --root .`.
5. Test native window, service health, conversation send and resume; a process surviving one second is **not** proof of readiness.

This patch **does not** merge Forge 08C, replace ForgeGUI pins, retire the internal PCC, start an LLM, certify the GUI, or mutate any unrelated repository. Those require the actual local source rollups, a persistent Forge operation service, and native Windows evidence before cutover.
