# Cortex W13B1 — marked Vault authority and guarded legacy adoption

**Published baseline:** `shifty81/Cortex` `8c75471fececa16dfb309f5b63f47cfb70d5120d` (W13A GREEN). This is one incremental W13B stage, **not** the complete PCC/Desktop shared-host consolidation.

## Why

The portable home on the actual `Vault` volume can contain a historical `registry/library.json` with `root` pointing to a different *live* physical drive. The prior Desktop rejects that state before displaying a window. A drive-letter replacement is not automatically a storage migration. D:, E: and G: in the supplied diagnostic are different physical volumes.

## Changes

- Source registry detects the marked home, makes no new unmarked `D:\Cortex` fallback, and resolves the portable installation before machine-local library configuration.
- Physical identity match permits remount-letter reconciliation when an existing record contains a GUID, or serial/size/filesystem evidence. Label alone never constitutes same-volume evidence.
- A record without prior identity and an unrelated live root remains blocked by default. The **one-time, explicit** environment approval `CORTEX_PORTABLE_ADOPT_LEGACY_ROOT=1` allows adopting the root on the marked, physically labeled `Vault` installation **only from that disk's own `.cortex/home`**, after the current root's marker is validated. An archival copy of the old library record is written under the *same portable home* before updating the record. It never copies, moves, erases or merges content on the former D:/E: root. If an old root has stable conflicting identity, even the one-time flag is insufficient.
- The Python PCC path resolver and Python chat/model bridge call the same storage-root function and reject a conflicting environment override when the installation is marked.
- Bootstrap checks the marked volume, physical `Vault` label and overrides before exporting paths.
- Windows and fake-volume regression cases distinguish portable and nonportable behavior.

## Application, source certification, live acceptance

1. Keep the W13A pushed commit as recovery baseline. Put only the *unextracted* patch ZIP and its `.sha256` sidecar in the Cortex root patch intake. The manifest verifies exact W13A file bytes before applying.
2. Use PCC intake/transactional application; restart when instructed. Run `cargo fmt --all` if FULL reports a Rust format-only diff, then FULL, and only then publish a new GREEN checkpoint. This package was inspected/tested in Linux Python, **not** compiled on Windows because Rust/MSVC and PowerShell are not available in the authoring environment.
3. Before the **one-time live adoption**, inspect the portable registry, marker, and Windows volume identity. Verify `Vault` is the volume containing Cortex and that old D/E are unrelated volumes. Keep a backup of `G:\.cortex\home\registry\library.json` independent of the automatic snapshot.
4. From one PowerShell session (adapt `G:` to the *actual* current letter):

```powershell
Set-Location 'G:\Cortex'
$marker = Get-Content 'G:\.cortex-volume.json' -Raw | ConvertFrom-Json
if ($marker.schema -ne 'cortex.volume.v1' -or $marker.volumeId -ne 'a18d6f5b-2ee0-46f8-8a57-731ba2f0c458') { throw 'Unexpected portable volume marker' }
if ((Get-Volume -FilePath 'G:\Cortex').FileSystemLabel -ne 'Vault') { throw 'This is not the labeled Vault volume' }
$env:CORTEX_PORTABLE_ADOPT_LEGACY_ROOT = '1'
try {
    # Only after W13B1 is source-certified. Launch via PCC/GUI or known executable.
    .\PROJECT_CONTROL_CENTER.cmd launch-gui
} finally {
    Remove-Item Env:CORTEX_PORTABLE_ADOPT_LEGACY_ROOT -ErrorAction SilentlyContinue
}
```

The flag authorizes **adoption of this marked Vault's existing portable-home authority record**, not a data migration. Check that the saved `library.json` now names the *current mount* of `Vault` and contains current volume identity; confirm the old record is archived beside it. Confirm no D/E files were moved. Subsequent launches must work **without** the flag, and a remount to a different letter must use the same marker and serial.

5. Certify independent direct `cortex_desktop.exe` launch, PCC launch, project inventory and simple chat. If the app still fails, capture the exact runtime log and stop; do not reset any state. `--certify` and a successful GUI window do not automatically prove provider, chat or project-creation certification.

## Remaining W13B/C work

One installation-level persistent host with multi-client leases; canonical PCC/Desktop project registry, conversation and job store; Desktop Home with attach/create flows; PCC chat UI; provider parity and HelloWorld_W12 end-to-end certification. W13B1 does not claim those are implemented.
