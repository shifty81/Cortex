# RS04–RS293 Cumulative Rollup

Baseline: RS03 GREEN `6d35ceea75dfa00bce392239fbce7a30eb2d9cc5`.

This package supersedes all earlier unbuilt post-RS03 cumulative handoffs.

## Cumulative stages

- **RS04–RS63:** normalize Cortex Desktop functionality into Forge-hosted Cortex workspace and lock Forge/Cortex/Ember hierarchy.
- **RS64–RS93:** establish persistent Forge machine/fleet state, nested project graph, active project, queue/service/source-control/takeover state.
- **RS94–RS193:** add native update, Artifact Central, VCS, scheduler, service, intake, certification, IDE backend, platform and release/recovery candidates.
- **RS194–RS293:** remove WebView dependency from IDE architecture, implement a native editor workspace, add native LSP/DAP framing, watcher/toolchain/diagnostics hardening, scheduler/VCS/release improvements and Ember host discovery.

## Important delivery rule

Apply **only the newest RS04–RS293 cumulative patch** to the RS03 GREEN baseline. Do not stack the older RS04–RS93 or RS04–RS193 candidate patches first.

## Authority

ForgePY remains production authority until the nested Rust candidate gate, runtime smoke, Cortex Full Gate and explicit takeover certification are all GREEN/VERIFIED.
