Extract/run the CMD anywhere. It automatically targets G:\Cortex if present.

Run:
    CAPTURE_CORTEX_PCC_LIVE_SOURCE.cmd

It does not modify source. It copies the exact live PCC control files, launcher,
project contract, Git status/diff/blob IDs, and current PCC logs into:

    G:\Cortex\artifacts\handoff\Cortex_PCC_Live_Source_<timestamp>.zip

Upload that ZIP to ChatGPT.

This is the correct next step because the live PCCSurfaceCommon.py has Git blob
26e187170781464ac053495f3c03546f49a1a0e5, which is neither the known stale
September-23 file nor the published d22 file.
