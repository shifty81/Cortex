# RS193 Home Build / Acceptance Handoff

## Apply

Use the single cumulative `forge.patch.v1` package bound to the RS03 GREEN commit. ForgePY remains the production update/build authority for this validation.

## Gate order

1. ForgePY Patch Review: validate project identity, package hash and RS03 Git precondition.
2. `forge-rust-gate` — fmt -> check -> all-target tests -> Clippy `-D warnings` -> `forge-rs` build.
3. If the nested gate fails, repair the **first** failing crate only and rerun the gate. Preserve architecture.
4. `forge-rust-build`.
5. `forge-rust-run` and smoke the existing GUI/Cortex host.
6. Run inspector/runtime checks for the new backends.
7. Cortex Full Quality Gate.
8. Commit + push GREEN to GitHub and snapshot to Forge Repository/Internal Git only after both gates pass.

## New backend smoke targets

After the candidate gate builds, exercise:

- `forge-patch-inspect` against the cumulative transport in validation-only mode;
- `forge-artifact-inspect` against configured Artifact Central;
- `forge-vcs-inspect` against Cortex;
- `forge-state-inspect` against Cortex;
- `forge-intake-scan` with Downloads auto-queue still disabled;
- `forge-takeover-status` and verify takeover remains false;
- safe `forge-ide-inspect` project binding;
- `forge-release-manifest` against a small staged directory before any full distribution packaging.

Do **not** use native Rust patch apply, Git push, scheduler mutation, service start/stop or release/recovery mutation as the first smoke action. Validate/read-only surfaces first, then exercise mutating operations one at a time after their unit tests are GREEN.

## Acceptance constraints

- no second Cortex Desktop window is required for the Forge Cortex workspace;
- existing project-owned PCC remains independently usable;
- ForgePY remains production authority;
- Downloads scanning cannot auto-apply a patch;
- Artifact Central promotion verifies bytes before authority promotion;
- patch apply checks target project and Git precondition before mutation;
- failed transactional patch apply restores already-modified files or reports rollback failure explicitly;
- Internal Git and GitHub/origin remain distinct lanes;
- takeover matrix remains fail-closed;
- no Ember editor feature is claimed implemented unless the real editor component exists.
