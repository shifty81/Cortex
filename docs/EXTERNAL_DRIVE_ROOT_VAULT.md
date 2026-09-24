# External drive root Vault

Cortex supports using an entire Windows drive root as the governed Vault authority.

For a portable laptop setup where the external drive is currently `E:`, keep the Cortex source in a normal source folder such as:

`E:\Cortex\`

Then run from that source folder:

`SETUP_EXTERNAL_DRIVE_VAULT_ROOT.cmd`

The setup binds the current drive root (`E:\`) to the user-scoped `CORTEX_VAULT_ROOT` override and creates the governed Vault namespaces directly under the drive root:

- `E:\projects\`
- `E:\objects\sha256\`
- `E:\shared\`
- `E:\quarantine\`

Additional project metadata and snapshots are created inside those governed namespaces as projects are registered and mirrored.

## Important boundary

Using `E:\` as the Vault root does **not** mean Cortex owns every pre-existing file on the external disk. Existing unrelated folders remain untouched unless the user deliberately registers/catalogs them through a project or future governed intake workflow.

The source itself should not be extracted directly into the drive root. Keep it in `E:\Cortex\` (or another project folder) so the project tree is distinct from the Vault storage namespaces.

## Drive-letter changes

Windows may assign the removable drive a different letter on another machine. If that happens, run `SETUP_EXTERNAL_DRIVE_VAULT_ROOT.cmd` again from the Cortex source located on that drive. The script derives the current drive letter automatically.

## Return to the default policy

Run `RESET_VAULT_TO_DEFAULT.cmd`, then restart the PCC/Forge GUI. The normal project policy will again select the configured default (currently the desktop D: policy when available, with local fallback otherwise).
