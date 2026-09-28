# W13B1B — Remaining Windows synthetic fixture isolation

Base: Cortex published commit `8c75471fececa16dfb309f5b63f47cfb70d5120d` + W13B1. This is an incremental patch. It does not overwrite W13B1 production storage code, and it may be applied after W13B1A because the three targeted files are independent of W13B1A's two changed test files.

## Evidence

`Cortex_DebugBundle_20260928-073945_FULL_FAIL.zip` fails in `python-pcc-regressions` at all-control-tests, with 315 tests, 3 errors and 2 skips. All errors are temporary non-portable fixture root overrides blocked by the real marked Vault authority. W13B1A did not touch these three files. The previously published W13A green marker is naturally stale while source is changed; do not use it as evidence of a passing current FULL gate.

## Change

- `test_pcc_console_chat_runtime.py`: mark synthetic bridge override as a non-portable fixture with a test-scoped mock.
- `test_pcc_portable_volume.py`: isolate the synthetic dependency-environment fixture, not the independent explicit conflict test.
- `test_pcc_vault_intake_audit.py`: explicitly isolate its temporary intake directory from the marked portable installation, using a scoped test decorator.

No production module, storage authority, registry, model host, or Vault contents are changed. No deletion/migration/adoption is included.

## Verification performed in staged W13A + W13B1 + W13B1A + W13B1B tree

- Control: `python -B -m unittest discover -s tools/control/tests -p test_*.py -b`: 315 tests, PASS.
- Inventory: 81 tests, PASS (1 skipped).
- Nested control: 4 PASS.
- Root staging: 5 PASS.
- Tools PCC: 5 PASS.
- Three modified Python files compile.
- Exact preimage SHA-256 is supplied for each changed file. Test-file preimages are the published W13A two unchanged files and W13B1-updated portable test file.

These are staged Python tests on Linux, not Windows FULL certification. Apply with regular Cortex PCC root patch intake. Restart PCC if prompted, run FULL, and share the next debug bundle if any stage fails. Do not run the one-time Vault authority adoption until the source is GREEN. Do not change or remove G:\.cortex or the existing D:/E: data.
