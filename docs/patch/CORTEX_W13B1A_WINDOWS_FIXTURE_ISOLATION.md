# Cortex W13B1A — Windows FULL-gate fixture isolation

## Exact failure addressed

The W13B1 root patch applied successfully on the user's physically marked `Vault` volume. Windows FULL failed during the Python control suite, before `cargo fmt`, because older PCC/Vault tests intentionally set their Vault root to unmarked synthetic temporary directories. W13B1's strict `resolve_vault_root` now correctly treats that as a conflicting override when invoked under the real portable `G:\Cortex` runtime. The failure log reports `Ran 315 tests`, `failures=7`, `errors=32`, `skipped=2`. Two additional environment-inheritance assertions unexpectedly selected the real shared build/toolchain paths rather than the synthetic fixture paths.

## Narrow correction

Only the following existing *tests* change; production code and Vault authority policy do not change:

- `tools/control/tests/test_w13_operational_recovery.py`: its temporary fixture explicitly models an unmarked/nonportable install via a scoped mocked policy. Its subprocess CLI smoke test sets `CORTEX_RUNTIME_ROOT` to the isolated fixture root so the child cannot accidentally load the real checkout's portable policy.
- `tools/control/tests/test_pcc_vault_storage.py`: its setup uses a scoped nonportable policy and clears inherited `CORTEX_PYTHON_EXE`/`CARGO_TARGET_DIR`, while directing shared-root preferences to the synthetic Vault. The test cleanup restores the original environment and policy. Tests covering *real* portable behavior in `test_w13b_portable_authority.py` are not modified.

The runtime remains fail-closed: a production portable Cortex with `.cortex-volume.json` cannot silently switch to D:/E:/another volume. Do not delete, rewrite or migrate `.cortex`, D: or E: state to resolve a failing test.

## Apply and certify

1. W13B1 must already be applied. Put this ZIP plus its identically named `.zip.sha256` sidecar in normal root patch intake (without extracting). The patch requires the exact preimages from published W13A for the two unchanged test files and an already applied W13B1 receipt.
2. Apply through PCC root-intake; restart the control session if requested; run FULL again. Source edits here are Python tests only, so a new Rust `cargo fmt` pass is not necessary unless the gate independently reports formatting.
3. Only after FULL GREEN and a backup/review of `G:\.cortex\home\registry\library.json`, perform the W13B1 one-time marked-volume authority adoption as specified in `docs/patch/CORTEX_W13B1_MARKED_VAULT_AUTHORITY.md`. That action is separate from this test repair.
4. Then run `launch-gui`, direct Desktop launch, and real chat/Hello World certification. FULL GREEN alone does not certify a visible GUI or model functionality.

## Evidence and limitations

On the authoring container: Python control discovery: 315 tests PASS; inventory: 81 tests, 1 skipped; nested project control: 4 PASS; root staging: 5 PASS. The Windows `rustc`/MSVC environment and physical Vault identity are not available in this container, so Windows FULL and GUI launch remain to be verified on the user's machine. This is a regression hotfix, not a claim that W13B2/C shared-host consolidation is complete.
