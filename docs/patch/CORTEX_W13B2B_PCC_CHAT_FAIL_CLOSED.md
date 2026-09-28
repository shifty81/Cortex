# Cortex W13B2B — PCC chat failure integrity

## Scope

Small follow-up to W13B2A, limited to PCC Python control-plane and its tests. Rust source, Vault, project registry, Desktop packages and conversation data are unchanged.

- Ordinary PCC Chat still reaches `cortex pcc-chat` when the common worker is present.
- A failed common-controller process, empty response or invalid JSON is reported as a failure. It cannot silently become a plausible non-mutating model reply.
- With no worker, actionable requests (including new project creation and approval/continuation) fail closed with explicit worker-build instructions; ordinary read-only questions can still use the previously available Python fallback.
- The PCC chat mode label identifies common Chat as a governed developer workflow, not always non-mutating.
- Bridge version and its test expectation updated together to 1.1.

## Applicability

- Apply **after** `CORTEX-W13B2A-PCC-ONLY-APPLICATION-CONSOLIDATION-20260928`.
- The ZIP uses exact W13B2A SHA-256 preimages for every replaced file and is rooted directly at the Cortex repository root.
- It is Python-only; the local `cargo fmt --all` on W13B2A Rust files does not alter its preimages.
- Do not extract manually into the source root or reapply W13B2A. Use normal governed PCC patch intake.

## Tests

Python `unittest discover` on an isolated source staging tree (Sep 28 rollup plus W13B2A overlay, then W13B2B edits): 313 PASS, including six new W13B2B tests. Rust formatting/build/FULL and live Windows PCC Hello World are not certified in this environment.

## Next live certification

Run FULL in PCC. If green, explicitly build the current Cortex worker through PCC (not native Desktop), check its selected path and use the PCC chat to create `CortexHelloWorld`, approve, then verify real source/build/executable output and project registration. Keep the resulting execution/debug bundle.
