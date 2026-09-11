# Forge Rust Parallel Lane

The existing standalone ForgePY remains the universal production build/control application. Rust Forge is developed in parallel under `products/forge-rust/` and is built through the Cortex project provider so ForgePY remains above the entire workflow.

```text
ForgePY (production)
  -> Cortex `forge.project.v1`
  -> ProjectControlCenter.py / CortexPCC.py
  -> forge-rust-gate / build / run
  -> products/forge-rust
```

No Rust Forge result can replace ForgePY authority until a dedicated takeover certification proves feature parity, recovery behavior, patch safety, source control, Artifact Central, tooling, Cortex integration, and multi-project operation.

The Rust GUI uses `egui`/`eframe` for the native shell. Monaco remains a separate future WebView window, matching the proven failure-isolation pattern used by ForgePY. Cortex and managed llama.cpp are integrated only after the deterministic Forge operation spine is certified.
