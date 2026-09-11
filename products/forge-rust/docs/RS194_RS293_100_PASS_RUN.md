# RS194–RS293 — 100-Pass Native IDE + Workstation Hardening Run

Baseline: RS03 GREEN `6d35ceea75dfa00bce392239fbce7a30eb2d9cc5`.  
Delivery: cumulative RS04–RS293 patch.  
Production authority remains ForgePY.

RS194. Lock the Forge IDE end-state to native Rust; WebView/Monaco is no longer required.
RS195. Remove active roadmap language that treats Monaco/WebView as a workstation-completion dependency.
RS196. Add `forge.native_ide.v1` capability truth including `webview_required=false`.
RS197. Add persistent-in-memory native editor session abstraction over the guarded IDE workspace.
RS198. Add multi-tab open/activate/close behavior with duplicate-open coalescing.
RS199. Add explicit dirty and external-disk-conflict state per editor buffer.
RS200. Add bounded editor undo history.
RS201. Add bounded editor redo history.
RS202. Harden document save to verify SHA preimage before mutation.
RS203. Harden save publication with temp verification, backup staging and rollback on publish failure.
RS204. Add bounded recursive text-file inventory for the active project.
RS205. Add IDE file-name/path filtering support in the Forge tab.
RS206. Add active-buffer text search with line/column results.
RS207. Expand language identification for Rust/Python/C++/JS/TS/HTML/CSS/YAML/RON and common project text.
RS208. Add language-tool configuration records.
RS209. Add runtime PATH availability checks for language tools.
RS210. Add Rust `rust-analyzer`/`rustfmt` suggestions.
RS211. Add Python `pyright`/`ruff` suggestions.
RS212. Add C/C++ `clangd`/`clang-format` suggestions.
RS213. Add JS/TS language-server suggestion contract.
RS214. Add native terminal profile discovery.
RS215. Render the Forge IDE as a real native egui workspace rather than a future-host notice.
RS216. Add native project file browser panel.
RS217. Add native IDE tab strip and active-tab switching.
RS218. Add dirty/conflict markers to native IDE tabs.
RS219. Add native Save action.
RS220. Add native Undo action.
RS221. Add native Redo action.
RS222. Add native disk-conflict refresh action.
RS223. Add native find-results panel.
RS224. Add active-language tool status display.
RS225. Bind IDE readiness into Forge hosted-workspace machine state.
RS226. Report native IDE startup health in Forge console.
RS227. Add `forge-protocol` crate for browser-free stdio protocol hosting.
RS228. Add `Content-Length` framed JSON writer.
RS229. Add framed JSON reader with mandatory content length.
RS230. Add 64 MiB protocol frame safety budget.
RS231. Add generic JSON-RPC request envelope.
RS232. Add JSON-RPC notification envelope.
RS233. Add matching-response request helper.
RS234. Add protocol child-process lifecycle ownership.
RS235. Add Windows no-console protocol-process launch mode.
RS236. Add drop-time protocol-process termination fallback.
RS237. Add LSP protocol-kind contract.
RS238. Add DAP protocol-kind contract.
RS239. Add generic JSON-RPC protocol-kind contract.
RS240. Add frame round-trip regression test.
RS241. Add oversized-frame rejection regression test.
RS242. Connect IDE language adapters to native protocol launch specs.
RS243. Preserve formatter/linter tools as process tools rather than falsely classifying them as LSP/DAP.
RS244. Add native IDE no-WebView regression assertion.
RS245. Add editor dirty/undo/redo regression test.
RS246. Add external-change save-conflict regression test.
RS247. Add `forge-toolchain` crate.
RS248. Parse project `requirements` from `project.control.json`.
RS249. Add PATH executable discovery.
RS250. Add version probe strategy.
RS251. Track required vs optional tool availability.
RS252. Add required-tool readiness result.
RS253. Add machine-readable toolchain report.
RS254. Add `forge-toolchain-doctor` binary.
RS255. Add Forge Settings Toolchain Doctor action.
RS256. Add missing-required-tool regression test.
RS257. Add `forge-diagnostics` crate.
RS258. Aggregate project-contract presence into diagnostics.
RS259. Aggregate toolchain health into diagnostics.
RS260. Aggregate Git health into diagnostics.
RS261. Aggregate Forge machine-state health into diagnostics.
RS262. Count durable operation receipts in diagnostics.
RS263. Add native overall-ready diagnostic truth.
RS264. Add machine-readable diagnostics binary.
RS265. Add Forge Settings native diagnostics action.
RS266. Add missing-project-contract diagnostics regression test.
RS267. Add `forge-watch` crate.
RS268. Add bounded filesystem snapshots.
RS269. Add configurable watch roots.
RS270. Add generated/cache directory ignores.
RS271. Add created-file change detection.
RS272. Add modified-file change detection.
RS273. Add removed-file change detection.
RS274. Add watch truncation truth.
RS275. Add persistent watch snapshot save/load.
RS276. Add `forge-watch-once` binary.
RS277. Integrate watch snapshots into patch intake.
RS278. Trigger intake reclassification only when `.patch` paths change.
RS279. Preserve Downloads non-auto-apply invariant.
RS280. Add scheduler policy model.
RS281. Add maximum queued items per run.
RS282. Add maximum selected parallel-job budget.
RS283. Add maximum mutation budget.
RS284. Add one-selected-mutation-per-project policy.
RS285. Add explicit scheduler defer reasons.
RS286. Expose future safe parallel batch plan without falsely claiming concurrent execution.
RS287. Keep first certified scheduler execution sequential until shared locking is proven.
RS288. Extend Git branch-name/ref validation.
RS289. Add guarded Git branch creation.
RS290. Add clean-tree guarded branch switching.
RS291. Add clean-tree fast-forward-only origin pull.
RS292. Add guarded commit-all receipt, annotated checkpoint tags and structured history rows.
RS293. Roll all RS04–RS293 candidate source, updated architecture, audit and home-build acceptance into one source-bound handoff.

## Additional cumulative hardening included in the same run

The numbered run is deliberately focused on the highest-risk path. Supporting changes in the same cumulative source also extend release update planning/recovery restore, tray/menu notification state, Ember host adapter validation/readiness, active documentation authority and takeover checks so the new IDE/workstation state is represented consistently.
