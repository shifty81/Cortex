# R7E.1 Cross-project volume discovery repair

Fixes the R7E doctor failure when invoked with a project under `<Volume>/projects`.

The caller path is now used only to discover `.cortex-volume.json`. The volume layout is loaded from the Cortex application authority at `<Volume>/Cortex/config/cortex/volume_layout.v2.json`, so individual projects do not need to duplicate Cortex configuration.
