# ForgePY -> Rust Forge Parity Matrix

| Surface | ForgePY production | Rust FR03 | Next authority pass |
|---|---|---|---|
| `forge.project.v1` | GREEN | HARDENED | compatibility-range negotiation |
| Project-native provider | GREEN | IMPLEMENTED | provider health/version handshake |
| Capability discovery JSON | GREEN-equivalent | IMPLEMENTED (`forge.capabilities.v1`) | expose in project registry |
| Quick Full Gate/Build/Run | GREEN | IMPLEMENTED | typed result/artifact bindings |
| Durable operation IDs | GREEN | IMPLEMENTED | cross-project scheduler identity |
| Operation receipts | GREEN | IMPLEMENTED (`forge.operation.receipt.v1`) | Artifact Central promotion |
| Persistent operation logs | GREEN | IMPLEMENTED | retention/search integration |
| Stop/cancel running operation | GREEN | IMPLEMENTED | cancellation escalation policy |
| Interrupted/crash truth | GREEN | IMPLEMENTED | process reattachment research |
| Operation history | GREEN | IMPLEMENTED | durable multi-project queue |
| Live project console | GREEN | IMPLEMENTED | structured stream channels |
| Semantic PASS/WARN/FAIL | GREEN | IMPLEMENTED | token-only rich rendering |
| Health rail | GREEN | PARTIAL | full status model |
| Projects/fleet | GREEN | ACTIVE PROJECT ONLY | registry/discovery |
| Updates/patch intake | GREEN | PROJECT PROVIDER ONLY | Rust transaction engine |
| Source Control | GREEN | PROJECT PROVIDER ONLY | GitHub + Internal Git |
| Forge Internal Git | GREEN | NOT YET | gix/native Git hybrid |
| Vault | GREEN | NOT YET | Artifact Central/catalog |
| IDE | native + Monaco | NOT YET | native recovery + Wry Monaco |
| Cortex | external status surface | BOUNDARY ONLY | integrated ChatGPT-style runtime |
| llama.cpp | Cortex-side donor | NOT YET | managed service/model registry |
| System tray | GREEN | NOT YET | tray-icon |
| Downloads watcher | GREEN | NOT YET | watcher + lineage triage |
| Artifact Central | GREEN | OPERATION EVIDENCE LOCAL | typed artifact index/promotion |
| Multi-project queue | planned | NOT YET | Rust durable jobs |
| Nested project graph | planned | NOT YET | workspace graph |
| Windows distribution | planned | NOT YET | installer/updater |

Rust Forge is a **candidate**, not production authority, until the complete takeover certification matrix passes.
