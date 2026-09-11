# Forge / Cortex / Ember Target Architecture

```text
+---------------------------------------------------------------------+
|                               FORGE                                 |
| universal native Rust workstation / project-control-tooling shell   |
|                                                                     |
| Projects | Workspace | Vault | Source Control | Native IDE | Ember  |
|                                                     |         |      |
|                                                     |         +-->  |
|                                                     |       game     |
|                                                     |     authoring  |
|                                                     |                |
|                                                     +--> Cortex      |
|                                                          workspace   |
+------------------------------+--------------------------------------+
                               |
                   versioned native contracts
                               |
                    +----------+----------+
                    |       CORTEX        |
                    | intelligence runtime|
                    +----------+----------+
                               |
            providers/models + agents/tools + context/memory
```

## Forge owns

- native application shell/navigation/health;
- project/fleet registry and nested project graph;
- project selection/workspace hosting;
- command/capability routing;
- build/test/run/package orchestration;
- durable operations/scheduling;
- Artifact Central and patch lineage;
- GitHub and Forge Repository/Internal Git;
- patch/update transactions and rollback;
- **native IDE/editor/tool integrations**;
- services/settings/tray/notifications;
- Ember host;
- release/recovery/takeover certification.

## Native IDE rule

The Forge IDE must not require Chromium, Electron, WebView2, HTML, CSS or JavaScript.

The current candidate uses egui-native presentation with a Rust editor/session backend and native stdio JSON-RPC framing for LSP/DAP-style processes. Future editor-core upgrades may reuse permissively licensed native Rust ideas/components, but the workstation must remain native and the IDE must continue consuming Forge project/source-control/build authority rather than creating parallel project-control systems.

## Cortex owns

- conversations/context/memory;
- providers/models/model lifecycle;
- Chat/Inspect/Plan/Apply/Repair;
- tools and Cortex permissions;
- safe streaming telemetry;
- Cortex jobs/tasks/activity;
- semantic project intelligence;
- Cortex review/approval intent;
- plugin/skill/protocol contracts;
- Cortex CLI/API/service contracts.

## Ember owns

- world/scene editing;
- content/assets;
- pixel/sprite/animation authoring;
- terrain/tile/level authoring;
- node/gameplay logic;
- UI/audio/dialogue authoring;
- engine/runtime/PIE authoring workflows.

Ember consumes Forge services and Cortex intelligence; it does not become the project-control backend.

## Project independence

Every managed project retains its own local CLI/PCC/provider and remains independently buildable/testable/runnable/certifiable without Forge. Forge discovers and invokes that authority through `forge.project.v1`.

## Production migration

ForgePY remains production authority until Rust Forge takeover certification is fully VERIFIED and explicitly approved. Candidate source or a single successful build is insufficient.
