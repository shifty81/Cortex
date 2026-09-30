# W13B2C1W — Windows workspace fixture correction

The original W13B2C1 Python unit test constructed a temporary bridge.py script by
interpolating a Windows `Path` inside Python double quotes. Windows paths such as
`C:\Users\...\Temp\...\new-project` contain escape sequences (`\U`, `\t`, `\n`)
when embedded as source literals and the child Python process exits 1. The test
mistakenly appeared to identify a broken workspace switch.

This narrow pass replaces the unsafe fixture interpolation with `repr(str(target))`
and adds an explicit Windows-path regression independent of the host OS. It changes
neither the production dispatcher nor the Rust controller, project authority,
conversations, model host, PCC GUI or Vault.

Prerequisite: W13B2C1 (terminal observability); compatible with W13B2C1P whether
or not it is already installed. The single existing test file has an exact preimage
SHA-256 check. FULL is still required on Windows after application.
