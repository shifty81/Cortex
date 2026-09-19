# U-PCC-A03 — cumulative controller-restart handoff repair

## Field evidence

Operator's Windows log: CTX-UPCC-A02-CUMULATIVE scanned and applied successfully, archived under `artifacts/patches/applied`, and emitted `RestartRequired: true`. The CLI wrapper `ProjectControlCenter.py` imported `CortexPCC.main()`; the controller raised `PCCRestart` after requesting a relaunch. The existing exception handler lived only under `if __name__ == '__main__'`, so the imported entry produced a traceback and returned exit code 1. The GUI displayed `patch-apply: FAIL` despite an applied patch.

## Implemented

- Keep the internal `PCCRestart` exception to break stale interactive loops after a self-update.
- Rename dispatch to `_dispatch_main` and make public `main()` catch *only* `PCCRestart`, returning success (exit 0) to imported callers, including ProjectControlCenter.py.
- Do not mask failed patch application, a failed attempt to spawn the replacement controller, or other exceptions.
- Add five focused regression tests: imported wrapper, interactive flow, ordinary failure, failed launch, and single restart request.

## Cumulative delivery

This handoff bundles A01 inventory, A02 universal plan/Python gate preflight and their tests/docs, plus A03 restart repair. It is **not a complete source rollup**. The A03 package marked `AFTER_A02` is preimage-guarded for the exact A02 source that was confirmed applied in the Windows log; the separate `FROM_GREEN` package targets the previously published SOURCE03R3 baseline. Do not queue both variants together.

## Interpretation

The patch-apply child process now returns 0 after a successfully *requested* control-plane relaunch. Exit 0 certifies patch apply and relaunch initiation, **not readiness of a new GUI window**. GUI lifecycle handoff and runtime handshake remain future distinct tests. A new Full Quality Gate and Windows GUI check are required following installation. The A02 archive should remain in applied history, never reapplied.

## Acceptance

1. Apply A03 through root PCC and inspect archive receipt.
2. Observe `patch-apply: PASS (0)` rather than traceback, assuming the new GUI process can be spawned.
3. Close stale GUI instance if one persists; confirm new controller starts and reports current project.
4. Run `python tools/control/CortexPCC.py self-test --root .` and `python tools/control/CortexPCC.py full --root .`.
5. Confirm A01/A02 audit and plan commands remain available.
