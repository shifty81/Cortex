CORTEX R8A PORTABLE BOOTSTRAP OVERLAY

Extract this ZIP directly over the existing portable Cortex source (for example E:\Cortex) and allow overwrite.
Then run BOOTSTRAP_CORTEX_ENVIRONMENT.cmd or PROJECT_CONTROL_CENTER.cmd.

Fixes:
- portable source at E:\Cortex automatically selects E:\ as Vault root
- rustup-init SHA-256 mismatch evicts stale cache and retries up to 3 times
- Rust/Git hydration failures no longer discard an already hydrated Python launch environment
- Windows Store python.exe execution alias is ignored as a real Python runtime
- regression tests are registered in Full Gate

Do not delete the old C:\Users\<user>\AppData\Local\Cortex\Vault bootstrap content until E:\ is verified healthy.
