# Forge Rust Parallel Lane

Production authority: **ForgePY**.  
Committed Rust candidate: **RS03 GREEN**.  
Current cumulative candidate: **RS04–RS293**, pending local build/certification.

Rust Forge lives under `products/forge-rust/` during migration and is built through the Cortex project provider:

```text
ForgePY (production)
  -> Cortex forge.project.v1
  -> tools/control/ProjectControlCenter.py
  -> forge-rust-gate / forge-rust-build / forge-rust-run
  -> products/forge-rust
```

The candidate contains contract/capability authority, durable operations, Forge-hosted Cortex, persistent machine state, fleet/nested projects, update transactions, Artifact Central, GitHub/Internal Git, scheduler/services, intake/lineage, takeover certification, release/recovery and a **native Rust IDE with no WebView requirement**.

RS194–RS293 additionally introduces native IDE session/UI state, language-tool discovery, LSP/DAP stdio protocol framing, filesystem/intake polling snapshots, toolchain doctor, native diagnostics, Git branch/history operations, scheduler resource planning and Ember host adapter discovery.

Still required for takeover:

- GREEN compile/test/runtime certification of the cumulative candidate;
- richer asynchronous LSP/DAP/editor integration;
- OS-native filesystem notifications/resumable large-drive scan;
- native Windows tray/toasts;
- installer/updater/signing/self-recovery;
- secure credential storage;
- actual installed ForgePY side-by-side semantic parity run;
- explicit takeover approval.

No Rust Forge result replaces ForgePY authority until the takeover matrix is fully VERIFIED and takeover is explicitly approved.
