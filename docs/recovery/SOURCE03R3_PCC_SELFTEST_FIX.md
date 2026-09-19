# SOURCE03R3: PCC stable version contract self-test repair

Restores the existing PCC semantic version to `CTX-PCC-12.0`. SOURCE03R2 inadvertently appended `+SOURCE03R2`, causing baseline `test_60_pcc12_version_and_no_duplicate_fast_menu` to fail even though the preceding Rust Full Gate passed. The source rollup feature and its CLI/menu integrations are otherwise byte-for-byte unchanged.

The patch requires the exact SOURCE03R2 CortexPCC.py preimage. If the file differs, stop and rebase; never disable checks. Run PCC `self-test` and then `full` after applying. The `full` source gate alone is not a substitute for the Python PCC self-test. Git setup/publish is a separate operation and this patch does not modify it.
