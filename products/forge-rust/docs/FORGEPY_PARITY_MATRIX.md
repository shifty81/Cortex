# ForgePY F60R17 -> Rust Forge Parity Matrix

| Surface | ForgePY F60R17 | Rust FR02 | Next authority pass |
|---|---|---|---|
| `forge.project.v1` | GREEN | HARDENED | compatibility-range negotiation |
| Project-native provider | GREEN | IMPLEMENTED | provider health/version handshake |
| Capability discovery JSON | GREEN-equivalent | IMPLEMENTED (`forge.capabilities.v1`) | expose in GUI/project registry |
| Quick Full Gate/Build/Run | GREEN | IMPLEMENTED | operation receipts |
| Live project console | GREEN | IMPLEMENTED | stop/cancel + log files |
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
| Artifact Central | GREEN | NOT YET | typed artifact index |
| Multi-project queue | planned F75 | NOT YET | Rust durable jobs |
| Nested project graph | planned F71 | NOT YET | workspace graph |
| Windows distribution | planned F79 | NOT YET | installer/updater |

Rust Forge is a **candidate**, not production authority, until the complete takeover certification matrix passes.
