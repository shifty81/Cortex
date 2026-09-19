# SOURCE03R2 — exact-source PCC source export integration

Target: the `CortexPCC.py` captured in `Cortex_DebugBundle_20260918-210544_FULL_FAIL.zip` (58,394 bytes; SHA-256 `803a8400514096645c9849cf1c605a00ee6ac139e8f6626e2de468cc84503d9e`). No older PCC implementation is substituted.

This patch adds `source-rollup` CLI, Source / Project Control menu option **70**, and Diagnostics menu option **17**. It uses the already installed SOURCE02 exporter, verifies ZIP content and checksum sidecar, and prints the actual archive path and SHA-256. Existing Git, update, menu, build and gate behavior is retained.

**Before applying**: Move *both* `Cortex_RootPatch_CTX_SOURCE03_PCC_ExporterMenu_20260918.zip` and `Cortex_RootPatch_CTX_SOURCE03_CorrectedDependency_20260918.zip`, plus their `.sha256` companions, outside both the repository root and `updates/inbox`. Do not extract or edit these ZIPs. They collide on the same patch ID and their expected PCC preimage belongs to another source generation. Use only the SOURCE03R2 replacement. Preserve old ZIPs for investigation; do not delete patch receipts.

After applying via PCC, run Full Gate and choose Source / Project Control → 70 or Diagnostics → 17. Headless: `python tools/control/CortexPCC.py source-rollup --root .`. Verify actual machine output, then upload the resulting source archive for source-accurate desktop/Full Gate repair. Rollup content excludes generated files, sensitive filenames and pending patch ZIPs; check the archive before sharing. This patch does not initialize Git or prove desktop functionality.
