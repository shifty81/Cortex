# RS94–RS193 — 100-Pass Cumulative Takeover-Backend Run

Baseline: `6d35ceea75dfa00bce392239fbce7a30eb2d9cc5` (RS03 GREEN).

These passes extend the unbuilt RS04–RS93 candidate. They are **Candidate**, not GREEN, until the Rust Forge gate and Cortex Full Gate pass locally.

## Passes

RS94. Re-audit the RS03 committed baseline and RS04–RS93 cumulative candidate before extending it.
RS95. Keep one cumulative delivery lane; RS04–RS193 supersedes earlier unbuilt cumulative packages.
RS96. Expand the isolated Rust Forge workspace with takeover-backend crates instead of coupling them directly into Cortex.
RS97. Add native `forge.patch.v1` manifest parsing.
RS98. Add optional transport SHA-256 sidecar verification.
RS99. Reject unsafe/rooted/traversal ZIP and project-relative paths.
RS100. Require exact manifest-to-ZIP payload membership.
RS101. Verify declared payload byte counts and SHA-256 digests.
RS102. Verify target project identity against `project.control.json`.
RS103. Verify required project contract schema.
RS104. Verify source-bound Git commit preconditions before patch apply.
RS105. Enforce add-vs-replace destination preconditions.
RS106. Stage complete patch payload before project mutation.
RS107. Create byte-verified backups before replacement.
RS108. Publish staged files through destination-local temporary files.
RS109. Rollback already-applied paths on transactional failure.
RS110. Persist structured patch receipts and retained transaction evidence.
RS111. Add machine-readable `forge-patch-inspect` validation utility.
RS112. Add patch-engine unit coverage for unsafe paths and manifest uniqueness.
RS113. Add project-scoped Artifact Central directory authority.
RS114. Add typed `forge.artifact.v1` evidence records.
RS115. Promote artifacts by copy -> hash verify -> destination promotion.
RS116. Separate pending/applied/failed/quarantine patch lanes.
RS117. Persist project artifact index evidence.
RS118. Add class-aware Artifact Central retention planning.
RS119. Add explicit retention/archive application.
RS120. Add machine-readable Artifact Central inspection utility.
RS121. Add Git working-tree inspection as a Rust Forge source-control backend.
RS122. Capture branch, HEAD and porcelain cleanliness.
RS123. Capture origin and upstream identity.
RS124. Capture ahead/behind synchronization state.
RS125. Parse and expose Git worktree inventory.
RS126. Create/ensure project-scoped bare Forge Repository/Internal Git repositories.
RS127. Bind/update the project working tree to a dedicated Internal Git remote.
RS128. Push explicit branch snapshots into Internal Git.
RS129. Add explicit GitHub/origin fetch operation.
RS130. Add explicit GitHub/origin push operation.
RS131. Add guarded worktree creation primitive.
RS132. Add machine-readable VCS inspection utility.
RS133. Turn the persisted multi-project queue into executable scheduler work.
RS134. Resolve each queued operation through the registered project root.
RS135. Use `ProjectSession` so scheduled work still invokes project-owned capability authority.
RS136. Launch scheduled work through durable `forge-process` operations.
RS137. Consume terminal operation events into durable queue terminal states.
RS138. Propagate process operation IDs into queue evidence.
RS139. Mark event-channel loss as interrupted rather than successful.
RS140. Add bounded `run_until_empty` scheduler execution.
RS141. Add scheduler result serialization and inspection utility.
RS142. Add typed universal service definitions.
RS143. Capture service stdout/stderr into per-launch logs.
RS144. Launch services through Rust Forge and persist PID/state.
RS145. Add persisted service health refresh.
RS146. Add cross-platform process-alive probing.
RS147. Add service stop/process-tree termination.
RS148. Fail service launch on invalid IDs/program/cwd.
RS149. Use Forge machine state as the service-status authority.
RS150. Add configured intake-root scanning for `.patch` transports.
RS151. Reuse native patch validation instead of duplicating manifest parsing.
RS152. Bind intake classification to current Git authority.
RS153. Recognize already-applied lineage records.
RS154. Recognize superseded lineage records.
RS155. Apply configured age-window stale classification.
RS156. Record invalid transports fail-closed.
RS157. Keep Downloads auto-queue disabled by default.
RS158. Require explicit approval before pending transport promotion.
RS159. Promote approved transports into Artifact Central pending authority.
RS160. Add machine-readable intake scan utility.
RS161. Add takeover certification schema and runner descriptors.
RS162. Add semantic JSON result comparison.
RS163. Support volatile-key exclusion for timestamps/ephemeral identifiers.
RS164. Require matching exit status as part of semantic parity.
RS165. Fail takeover certification closed when comparisons are empty or divergent.
RS166. Persist certification evidence back into the takeover matrix.
RS167. Keep takeover impossible unless every required check is Verified/NotApplicable.
RS168. Add machine-readable takeover-status utility.
RS169. Add safe native Forge IDE workspace authority.
RS170. Reject IDE absolute/traversal paths.
RS171. Open bounded UTF-8 project documents.
RS172. Detect source language from file type.
RS173. Require SHA-256 preimage match before IDE save.
RS174. Save through a destination-local temporary file.
RS175. Add bounded project text search.
RS176. Skip generated/heavy operational directories during IDE search.
RS177. Add machine-readable IDE host inspection utility.
RS178. Add Forge platform/tray/notification settings contract.
RS179. Add durable notification records and levels.
RS180. Persist/read platform notification state independently of project source.
RS181. Add read/unread notification lifecycle.
RS182. Keep native tray implementation Candidate rather than falsely Verified.
RS183. Add portable release manifest generation.
RS184. Hash every governed release file.
RS185. Verify release package bytes and hashes.
RS186. Add explicit recovery-checkpoint creation.
RS187. Reject unsafe recovery-relative paths.
RS188. Add machine-readable release-manifest utility.
RS189. Make Forge settings forward-compatible with serde defaults.
RS190. Add bounded intake polling and scheduler concurrency settings.
RS191. Expand takeover matrix with patch engine, intake, IDE, platform and release candidates.
RS192. Normalize active architecture/roadmap/quality-gate documentation to the RS193 candidate truth.
RS193. Produce one source-bound cumulative patch plus acceptance handoff for the home build.

## Implementation strategy

The run deliberately puts the new takeover authorities into separate Rust crates. This keeps the existing Forge host/Cortex normalization stable while giving the first home build small, attributable compiler/test surfaces.

New candidate crates:

- `forge-update` — native modern patch validation/transaction/rollback/receipts;
- `forge-artifacts` — Artifact Central project layout, verified promotion, index and retention;
- `forge-vcs` — Git working state, worktrees, Internal Git and GitHub/origin command backend;
- `forge-scheduler` — persisted queue execution through project-owned capabilities and durable operations;
- `forge-services` — process lifecycle/logging/health bound to Forge state;
- `forge-intake` — safe intake classification and explicit Artifact Central promotion;
- `forge-certify` — fail-closed semantic takeover comparison foundation;
- `forge-ide` — safe file/document/search/preimage-save core;
- `forge-platform` — durable tray/notification/settings state contract;
- `forge-release` — release manifests and recovery checkpoints.

## Truthful limits after RS193

- Native system-tray rendering and Windows toast integration are still not implemented; only durable platform state/contracts exist.
- Monaco/WebView hosting is still not implemented; `forge-ide` is the backend authority only.
- The scheduler executes persisted jobs sequentially; parallel scheduling policy exists only as a bounded setting.
- Intake is a scan/poll backend, not yet a native filesystem-notification watcher.
- Git operations use the installed Git executable from Rust; replacing read-heavy operations with `gix` remains optional optimization work.
- The update engine supports current Forge `add`/`replace` transports; schema-level deletion must be added only when the transport contract formally defines it.
- Release manifests/recovery checkpoints do not equal an installer/updater.
- ForgePY remains production authority until semantic parity certification and explicit user approval.
- Ember remains downstream real editor work; no fake Ember authoring implementation is introduced here.
