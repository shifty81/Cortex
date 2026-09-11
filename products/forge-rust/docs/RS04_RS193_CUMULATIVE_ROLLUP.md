# RS04–RS193 Cumulative Rollup

Committed baseline: RS03 GREEN (`6d35ceea75dfa00bce392239fbce7a30eb2d9cc5`).

The cumulative candidate now contains three major layers:

1. **RS04–RS63 — Forge-hosted Cortex normalization**: real Cortex Desktop-core reuse, conversations/agents/files/context/provider/activity/Vault/tasks/settings/repository operations, plus Ember/IDE host boundaries.
2. **RS64–RS93 — persistent Forge machine-state spine**: fleet/nested graph, active project state, Artifact Central lineage state, services/queue/source-control/host-workspace records and takeover matrix.
3. **RS94–RS193 — executable takeover backends**: native modern patch engine, Artifact Central promotion, VCS/Internal Git, scheduler, service lifecycle, intake triage, takeover comparator, IDE backend, platform state and release/recovery evidence.

## Authority invariant

ForgePY remains production authority. Rust Forge is still a candidate even if it compiles once. Projects keep their own CLI/PCC/provider. Cortex remains the intelligence authority. Ember remains the game-authoring authority.

## First build rule

Do not apply older unbuilt RS04–RS93 packages and then stack this package. The RS04–RS193 package contains the entire post-RS03 candidate lane and should be applied directly to the RS03 GREEN baseline.
