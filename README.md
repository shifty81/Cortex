# Cortex N1 — Native Desktop Startup Truth (review-only source candidate)

**Date:** 2026-09-21. **Product:** Cortex, NOT Forge. **Authoritative source reference:** `shifty81/Cortex` commit `5e5e113d6168acac721bb722d169be04b4ec1c3f`; `tools/control/CortexPCC.py` Git blob `9c9f611cf70bbaaffc62b9375c0a05bebe11cc06`.

## What is implemented

- `tools/control/CortexDesktopReadiness.py`: a bounded Windows-only check for a visible, reasonably sized top-level GUI window owned by the exact PID launched by Cortex PCC. Detects early exit (even exit 0), process/window probe errors, and a living but windowless process. Doesn't execute other programs, write files, kill processes, start providers, or treat a window as a chat-ready handshake.
- `tools/control/tests/test_cortex_desktop_readiness.py`: ten deterministic tests, including early exit, timeout, error handling, delayed window, no shell/process execution, and honest status.
- A review-only change to `tools/control/CortexPCC.py`: retain typed-contract preflight and existing build/launch path; replace `time.sleep(1.0)` with the bounded PID-specific window observation. Full Gate's *existing* mandatory Python suite includes the new test.
- `stage_candidate.py`: fail-closed source fingerprint check and output to a NEW directory **outside** the real Cortex repository, with changed files, generated diff and provenance. Doesn't modify the checkout, create a branch, run a gate, touch patch intake or push to GitHub.

## Why not a normal Cortex `.patch` yet?

The user's actual local Cortex source, including dirty/untracked files, is **not** available to this candidate. A full Forge C11 source ZIP is not a replacement. The stager checks the *exact published PCC blob* and refuses other revisions, even if the source looks similar. It makes **review files only**, not a transport accepted by Cortex's transactional patch authority.

## Optional staging on the local PC (non-mutating)

Extract this candidate ZIP **outside the source checkout and outside all patch inboxes**. In a terminal within that extracted candidate directory:

```powershell
py -3 stage_candidate.py --root "C:\Users\Shifty\Desktop\Cortex-main" --stage "C:\Users\Shifty\Desktop\Cortex-N1-Review"
```

Replace the root with your *actual* Cortex path. `--stage` must name a new directory that does not exist. The script refuses unknown PCC preimages, already-existing module paths, non-Cortex roots and in-repository destinations. If it refuses, **do not manually force its changes**; use the actual source for a rebase.

Review `Cortex-N1-Review/review/CortexPCC.diff` and the three staged source files. The original Cortex tree is unchanged. This kit should not be dropped into root patch intake and should not be treated as installed. To prepare a governed patch for the actual checkout, supply its existing `CortexSourceRollup.py` archive + Git state + current PCC Full Gate/runtime log. The existing source exporter is:

```powershell
# Run from real Cortex root; this writes a source archive, not a source patch.
python tools/control/CortexSourceRollup.py create --root .
# Verify using the *exact printed archive filename*:
python tools/control/CortexSourceRollup.py verify --archive "<actual-created-archive-path>"
```

## Local isolated tests

```powershell
py -3 -B -m unittest -v test_stage_candidate tools/control/tests/test_cortex_desktop_readiness.py
```

These tests exercise this candidate, not Cortex's full Python gate or Windows GUI. `cargo`/`rustc` and Windows GUI are unavailable in the authoring environment, so no native build or desktop runtime certification is claimed.

## Acceptance before N2 (ForgeGUI UI host)

1. Current checkout fingerprint and source/asset backup established; candidate rebased or exact preimage confirmed.
2. Existing `contract-migrate` PREVIEW reviewed; no automatic application. If approved, run source-guarded migration and prove backup/receipt.
3. Real Windows `CortexPCC.py full` GREEN on exact source; start Cortex and observe `WINDOW_VISIBLE` with a runtime log. A timeout/early exit must FAIL, never GREEN.
4. Manually send a real prompt with the configured provider, observe stream + response, restart and recover same conversation, cancel an agent, verify error-path UI.
5. Pin and independently build ForgeGUI's `forge_gui_consumer_starter`; only then implement `DesktopHost` presentation adapter, retaining old Win32 host as fallback.

**Do not rename C11 Forge into Cortex, replace Cortex Desktop, or introduce a second conversation/operation authority.**
