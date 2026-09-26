# R7A Repository + Portable Volume Convergence

This is the safe migration layer, not a blind delete pass.

Run PLAN first:
`python tools\control\CortexRepositoryConvergence.py --root G:\Cortex`

Then APPLY:
`python tools\control\CortexRepositoryConvergence.py --root G:\Cortex --apply`

The apply path copies and SHA-256 verifies runtime/history content before removing its old repository copy and writes a receipt under `.cortex/receipts/repository-convergence`.

It migrates:
- `data/conversations` -> volume `.cortex/conversations`
- `data/registry` -> volume `.cortex/registry`
- `logs` -> volume `.cortex/logs`
- `migration` -> `Vault/history/Cortex/repository-convergence/<stamp>/migration`
- obvious root repair/handoff/manifest residue -> the same Vault history receipt set.

It deliberately leaves currently-referenced bootstrap/hydration wrappers and `products/forge-rust` in place for the next reference-rewrite pass.
