# ForgePY / Python PCC -> Rust Forge Takeover Matrix

**Candidate:** RS04–RS293 cumulative lane  
**Production authority:** ForgePY / Python PCC  
**Rule:** implemented source is not takeover parity until side-by-side behavior is verified.

| Surface | Production authority | Rust Forge RS293 candidate | Remaining before takeover |
|---|---|---|---|
| `forge.project.v1` / project provider | GREEN | implemented/hardened | installed-project compatibility fixtures |
| Build/test/run/gates | GREEN | implemented | result/artifact semantic parity + runtime smoke |
| Durable operations/logs/cancel/history | GREEN | implemented | crash/restart/concurrency parity |
| Interactive project console / command composer | GREEN | live output implemented; input/send parity still pending in Rust candidate | mirror PCC-GUI 0.11 command catalog, Cortex routing, manual project CLI, history/help/stop and compact/verbose log behavior |
| Project/fleet registry | GREEN | persistent registry + nested graph | large-drive/resumable discovery proof |
| Patch/update transactions | GREEN | native transaction/rollback candidate | mutation parity + recovery fixtures |
| Vault / Artifact Central | GREEN | promotion/index/retention candidate | shared-Vault/PCC interoperability + runtime parity |
| Patch lineage/intake | GREEN | classification/promotion/watch snapshot | OS-native events + installed ForgePY parity |
| GitHub/Internal Git | GREEN | inspect/worktree/snapshot/branch/pull/commit/tag/history candidate | rich GUI + semantic parity fixtures |
| Scheduler/jobs | GREEN | durable execution + budget planner | certified concurrent worker/locking behavior |
| Services | GREEN | start/stop/status/log candidate | dependency/recovery behavior |
| Toolchain doctor | GREEN | requirement/PATH/version doctor | install/remediation workflows |
| Diagnostics/debug evidence | GREEN | native aggregated report | complete debug-bundle/evidence parity |
| IDE | production tooling | native egui editor + safe save + protocol transport | large-file buffer, syntax, async LSP/DAP, terminal, richer UX |
| Cortex integration | production Cortex | host connection/contracts candidate | real conversation/provider/runtime acceptance |
| Downloads/watch intake | GREEN | watch snapshot/classification candidate | native event stream + lineage triage parity |
| Platform notifications/tray | GREEN | command/state candidate | native Windows tray/toast runtime |
| Release/recovery | production procedures | manifest/update plan/recovery checkpoint candidate | installer/signing/updater/self-recovery |
| Credentials | production behavior | no fake persistence | OS-backed secure store |
| Takeover decision | authoritative | fail-closed candidate matrix | all required rows VERIFIED, not merely implemented |

Rust Forge remains a **candidate**, not production authority, until the complete takeover certification matrix passes on the target workstation.
