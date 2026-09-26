# R7C Actual Portable Volume Authority

Supersedes the proposed R7B destination assumptions.

The existing portable drive layout observed by the operator is authoritative. Cortex discovers the mount using `.cortex-volume.json`, then resolves named sibling authorities from `config/cortex/volume_layout.v2.json`.

Notable corrections:
- `Artifacts` is a top-level authority, not `.cortex/artifacts`.
- `Git` is a top-level authority, not `.cortex/git`.
- `Vault` is `/<mount>/Vault`; the mount itself is never called the Vault.
- `shared` is `/<mount>/shared`; it is not nested under Vault.
- existing `projects` casing is preserved.
- `.cortex` is state only.
- `Cortex` is source/application checkout only.

This pass intentionally does not move files. It first corrects path authority so subsequent migrations cannot create another duplicate hierarchy.
