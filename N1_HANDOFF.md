# N1 bounded implementation result

- **Source reference:** GitHub Cortex 5e5e113d6168acac721bb722d169be04b4ec1c3f; exact PCC Git blob `9c9f611cf70bbaaffc62b9375c0a05bebe11cc06`.
- **Implemented here:** PID-bound visible Win32 window observation; fail-closed early-exit/timeout/error statuses; integration change for existing `launch_gui`; mandatory Full Gate Python test registration; safe external staging + source fingerprint/provenance; focused tests.
- **Not installed:** This is not an overwrite-capable PCC `.patch` or a current local-source rollup. No user checkout was changed. No Windows launch/build or chat provider was certified.
- **Next action for actual source:** run the existing Cortex source exporter; compare local source hash, contract and patch receipts; build a governed Cortex-root patch and run Windows Full Gate. If exact published PCC file is present, the stager creates reviewable files without writing into source.
- **N2 code destination:** optional Cortex-owned `DesktopHost`/`DesktopWorker` presentation adapter consuming pinned `forge_gui_shell` through public APIs; retain original Win32 host, controller, conversation persistence, agents, PCC and model/provider paths.
- **N2 acceptance:** real chat rendered in ForgeGUI with one original conversation ID and original controller, Windows compile/smoke, working send/stream/cancel/restart, independent ForgeGUI consumer gate, layout persistence/DPI and no fake controls.
