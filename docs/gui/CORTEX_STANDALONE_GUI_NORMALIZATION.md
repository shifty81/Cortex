# Cortex Standalone — GUI Normalization and Chat/Console Separation

**Status:** implementation specification; NOT an implemented patch or runtime certification.  
**Source baseline:** `shifty81/Cortex`, `main`, commit `2ecf5b1b2c98b5e571504b892c4b4649659ced02` (the user's published source-recovery checkpoint).  
**Primary existing surface:** `tools/control/CortexPCCGui.py` (`PCC-GUI-0.10.1` in the examined source).  
**Decision:** this becomes Cortex's standalone workspace shell, not an embedded Cortex tab owned by Forge or a project-local PCC. Other projects connect via project adapters and supported local APIs.

## 1. Corrected product decision — chat REPLACES Dashboard, not Console

The Project Workspace uses four *simultaneously visible and independent* panes:

| Pane | Target share of available workspace body | Default ownership |
|---|---:|---|
| Project Operations (left) | 10% | Nav, approved action controls, active operation |
| Cortex Chat (center-left) | 40% | Project-scoped conversational developer UI, composer, durable thread |
| Project Console (center-right) | 40% | Read-only-by-default build/test/run/tool output and diagnostic stream |
| Health (right) | 10% | Git, GREEN, patch queue, PCC, runtime, provider, sync; actionable detail |

`Dashboard` is **removed as the default center-left page**. Project authority/status is surfaced in a compact context header at the top of Chat, the right Health rail, and the Projects inspector. Do not recreate a full Dashboard in the chat panel. Clicking left-hand operations opens a transient inspector/drawer or an optional subview; it must not destroy or remount the chat conversation/composer or clear console output. The drawer is dismissible and returns the uninterrupted Chat view. Users can optionally show a contextual action/detail overlay while Chat and Console remain the owning persistent panes.

**Chat and Console are not interchangeable.** Conversation text, AI responses, file/diff review, approvals, model and provider controls belong only to Chat; raw process stdout/stderr, structured operation events, filters and log controls belong only to Console. A conversation may *reference* a build by operation ID and summary, and the console may show a correlation ID, but the same text should not be duplicated wholesale into both panels.

### Workspace schematic

```text
┌──────────────────────── Application title/header ───────────────────────┐
│ Cortex · active project / project selector             app actions      │
├──────────────────────── Global navigation ─────────────────────────────┤
│ Projects │ Project Workspace │ Chat                    Vault / Forge    │
├────────────────────── Contextual action bar ────────────────────────────┤
│ Active project actions: Full Gate · Build · Run · Apply · Debug · Export  │
├───────────┬──────────────────────┬──────────────────────┬────────────────┤
│ OPERATIONS│ CORTEX CHAT          │ PROJECT CONSOLE      │ HEALTH         │
│   10%     │        40%           │         40%          │   10%          │
│           │ Context / thread     │ Operation filter     │ Git            │
│ [Actions] │ Rich messages        │ stdout / stderr      │ GREEN          │
│ [Jobs]    │ Tool/diff/approval   │ structured log       │ Updates        │
│ [Stop]    │                      │                      │ PCC            │
│           │ Composer             │                      │ Runtime        │
│           │                      │                      │ Provider/Sync  │
├───────────┴──────────────────────┴──────────────────────┴────────────────┤
│ Persistent bottom status bar · project · session · running operation     │
└─────────────────────────────────────────────────────────────────────────┘
```

The percentages are defaults across the *usable content width*, excluding panel sashes, paddings and borders. They are not four independent screen-width hard codes. Save user changes per app or per workspace. At narrow widths, collapse Health into an icon rail or popover, then Operations into compact icon mode, before compromising Chat and Console minimum usable widths. Do not animate or repeatedly rewrite sash positions during ordinary window resizing; initialize once after the workspace is laid out, persist only after user-initiated sash changes, and clamp to current viewport before restoring.

## 2. One consistent application shell, not one identical page body

The outer shell is invariant across Projects, Project Workspace, Chat and Vault / Forge:

1. **Window chrome / title:** Cortex application identity and selected project; app version, refresh, safe launch/open-CLI actions and system window controls. No status-light strip in this row.
2. **Global navigation:** left `Projects | Project Workspace | Chat`; right `Vault / Forge`. All must be real buttons whose focus/selection/accessibility state corresponds to displayed content. Switching pages must not shut down Cortex services or discard a chat draft.
3. **Contextual action bar:** one consistent height, spacing and command placement, but actions change with the active global page. Destructive actions isolated on the far right; busy/permission gating visible.
4. **Page body:** page-specific arrangement. The 10/40/40/10 rule is mandatory for **Project Workspace**, not for the Projects registry table or global Chat library, which need page-appropriate space.
5. **Bottom status bar:** always present; concise operation, connection and error summary. Never use statusbar text as the only place a failure is reported.

Global nav, contextual bar and status bar retain consistent token styling: palette, border, radius, typography, focus ring, spacing, minimum hit target, hover/pressed/disabled states. The top bars do not jump in height when tabs or projects change. Use one centralized definition of visual tokens and shared shell widgets; no duplicate per-page chrome or arbitrarily embedded titles.

## 3. Projects page — library/registration manager

**Purpose:** discover, register, inspect and open projects, not run a chat in the registry table.

- Left-aligned safe actions: `Register Project…`, `Open Project Workspace`, `Open Folder`, `Rescan / Rebind`.
- Right-aligned isolated danger action: `Remove Registration` with selection check and explicit confirmation explaining that registration is removed **without deleting project files**. Disabled when no valid selection.
- Main body: stretchable/sortable project table, responsive columns and sensible minimum widths; visible status difference between `missing root`, missing PCC, not scanned and valid; double-click opens workspace only when root exists.
- Bottom: Selected Project inspector showing resolved root, project identity, adapter/provider, Git summary, Vault scan status, last-opened time and rebind/recovery guidance. Don't silently remove stale rows or treat a missing root as deletion.
- Selection and vertical scroll positions survive tab navigation and refresh. Rescan/rebind runs off the UI thread and shows progress/cancellation and actual status.
- Optional `Open Cortex Chat for This Project` in inspector is a safe contextual action; it opens a project-bound conversation in the global Chat page. The main Projects toolbar remains uncluttered.

**Direct correction in current source:** `_build_projects_tab` currently packs `Remove Registration` between Open Folder and Rescan. Move the danger button into a right-packed group and preserve normal actions on the left. The current project registry table and selected-project details can be retained instead of rewritten.

## 4. Project Workspace — operational developer desk

**Panes:** Operations | Chat | Console | Health, as specified above. Chat is the default center-left content at every activation. No `Dashboard` tab is needed.

**Left Operations:** compact command categories (Build & Run, Updates, Source Control, Diagnostics, Advanced), active-job indicator, safe Stop/Cancel. Replace `Dashboard` nav entry with a Chat/Overview shortcut that focuses the composer. Clicking categories opens a contextual, dismissible command-details drawer, *not* a new global app tab. Tool actions must use the selected project's PCC provider rather than hardcoded Cortex commands. Missing/unsupported commands show unavailable metadata instead of fake clickable controls.

**Chat:** shared conversation service, scoped to the actual active project ID and canonical root; thread header includes project, conversation, model/provider/connection state; streaming messages, markdown/code, previews/diffs, tool calls, approvals and a multiline composer. Support sending, stop generation, draft persistence, scroll anchoring and visible errors. Clear console must never clear chat. Chat can ask PCC to run a registered build/gate, but may not skip approvals, transaction/rollback, or GREEN/Git checks.

**Console:** existing live stdout/stderr and PCC log pipeline remains and is improved, not replaced. Support bounded buffer/virtualized history, timestamps, operation ID, severity, stage/source filters, follow-tail toggle, independently scrollable view, Copy Selection/All, Clear View and Open Log. Clear View only clears rendered view; logs stay on disk. Never synthesize a green launch from process spawn or brief survival: read actual startup/health handshake and preserve stderr/exit diagnostics.

**Health:** move header LEDs into a vertical panel, each with label, state and click-through detail: Git, source GREEN, pending/invalid patches, root hygiene, PCC/provider, Cortex runtime, model/provider, optional Sync. Use PASS/WARN/FAIL/UNKNOWN text plus color and tooltip; distinguish build/source certification from GUI readiness. Show stale-green status when fingerprint changes. Failed runtime must not display `Ready` solely because Cargo built.

**Quick actions:** retain existing frequently used actions and add `Create Source Rollup` (calls the already working project-local `source-rollup` command and verifies it); build/run/patch controls disabled appropriately while a conflicting job is active. The source rollup is an artifact, never an update transport.

## 5. Global Chat — ChatGPT-style conversation home

Navigation `Chat` is a **full page** for all Cortex conversations (global and all projects). It is not a second independent chat backend and not the Project Workspace's console. Default page design: conversation browser/sidebar (~22%), active thread (~58%), optional context inspector (~20%). Context inspector may collapse; conversation sidebar supports expand/collapse, search and filters.

Conversation browser: New Chat, New Project Chat, all/recent/pinned/archived, project-grouped conversations, last activity and filter/search; add rename/archive/delete with confirmation and undo where supported. Data must be sourced from the existing persistent conversation storage, not synthesized from filenames or duplicated as a separate GUI-only database.

Active thread: same messages and identity used by the Workspace Chat pane. A project-bound conversation opened in either surface continues in the other at the same persisted message ID. Project switch does not silently redirect a chat's project/root or migrate conversation state. General chat is explicitly unbound and must not receive implicit source-write tools. History, drafts and selection survive page changes and window restarts where persistence is available.

Context inspector: attached project, approved tools/permissions, model/provider, files/artifacts and operation links. A direct `Open in Project Workspace` action selects the bound project and exact conversation. If its project root has moved/missing, surface rebind/recovery instead of silently opening a similarly named project.

## 6. Vault / Forge page

Remain on the right side of global navigation. Retain existing Vault catalog/tree, metadata, provenance and scanning capabilities. Use normalized header/action bar/panel skin. Scans must be cancellable, async, deterministic and display classified paths without unintended mutation. When integrated with Forge, show ForgePY as active universal front end; Rust Forge takeover is not assumed. Avoid embedding a second complete Cortex application or hardcoding `Cortex-main` into other projects.

## 7. Runtime architecture and ownership

`Cortex Core` owns conversation persistence, project binding, model routing, agent execution, tool permission checks and the IPC/RPC contract. `Universal PCC` owns project operations, build/test/gates, source fingerprints, patch transactions, Git, logs and artifacts. The standalone shell is a client/composition layer for both; other repos install their own provider/adapter and connect to the same standalone Cortex instance.

Reuse the existing `crates/cortex_pcc` typed provider bridge, `crates/cortex_rpc` loopback transport and `crates/cortex_conversation` persisted conversation authority. Do not embed a foreign native window inside Tk Text; instead expose proper conversational API/event contracts and render the Chat view in the selected shell. The existing `tools/control/CortexPCCGui.py` is a viable transitional shell; a future native Rust UI should consume the same versioned view-models, not fork conversation storage or command logic.

Versioned calls/events to define or finalize: `service.health`, `conversation.list/create/open/send/stream/cancel`, `project.bind`, `pcc.catalog/run/gate/status`, `operation.subscribe/cancel`, `artifact.open`, `review.approve/reject`. **These are proposed interface names, not an assertion that they all exist now.** All project mutations must be authorized in the controller, never based only on GUI button states. Use a local authenticated loopback endpoint, capability/version negotiation, per-workspace identity and correlation IDs, no arbitrary model-generated shell argv, and no recursive Cortex→PCC→Cortex operation dispatch.

## 8. Existing bug: startup contract mismatch

Current screenshots show `unknown variant 'git_or_snapshot'` while Desktop exits almost immediately and PCC records a launch PASS. In current published `project.control.json`, `fmt.apply` still declares `rollback: git_or_snapshot`; the Rust type `RollbackPolicy` recognizes `none`, `snapshot`, `transactional`, `provider_owned`. The same manifest declares `process_stop` for `forge.rust.run`. Fix by selecting and implementing correct semantic rollback policies, then validate the complete manifest against typed contracts in PCC's source gate and executable startup. Do not blindly change strings to appease deserialization while losing rollback functionality. Introduce a runtime ready handshake (correct project identity, supported contract version, initialized services, UI-ready/window-visible evidence), bounded timeout and early-exit capture. Only that state may drive Health: Runtime Ready / launch PASS; raw process-spawn result is a separate status.

## 9. Layout/state contract

- Define stable pane identities: `operations`, `chat`, `console`, `health` and global destinations `projects`, `workspace`, `chat_home`, `vault_forge`.
- Default `workspace.width_weights = [0.10, 0.40, 0.40, 0.10]`; save user-resized weights, clamp minimum widths, and restore once after actual layout dimensions are available.
- Approximate desktop min widths at 1280 logical pixels: operations >=120; chat >=380; console >=380; health >=110, with separators/padding taken from remaining width. Below this point compact/collapse edge rails first, never let chat composer or console disappear offscreen.
- Window resizing is continuous and jitter-free: no forced geometry feedback loops in Configure callbacks; debounce persistence or persist only on drag release; don't set sash positions every refresh, tab switch, health update or stream event.
- Split scrolling: chat scroll does not scroll console; console follow-tail doesn't steal composer focus; health inspector scroll independent.
- Persist global page, selected project, chat ID per project, drafts, pane sizes and optional drawers without writing into governed project source. Use an app-local settings/state path with explicit schema/version and safe migration.
- Keybindings: Ctrl+L focus Chat composer, Ctrl+K conversation/command search by active page, Escape dismiss contextual drawer, Ctrl+Enter send (optional configurable), Ctrl+F search current panel. Avoid hijacking existing platform shortcuts.
- Accessibility: state not conveyed by color alone, keyboard focus and contrast, real hit targets and horizontal layout behavior at 100–200% DPI.

## 10. Source-exact file and module plan

| Current source | Intended change |
|---|---|
| `tools/control/CortexPCCGui.py` | Preserve existing project registry, console and process host; change four-pane composition, nav, page action bars, health location and chat view integration. Extract shared visual widgets instead of growing a monolith. |
| `tools/control/PCCSurfaceCommon.py` | Keep project identity/registry/provider resolution canonical. Extend contracts only for missing typed UI status or operation metadata; don't duplicate Cortex service logic. |
| `tools/control/CortexPCC.py` | Retain authoritative `source-rollup`, gate, patch and Git entry points. Add no chat storage or UI-specific logic here. |
| `crates/cortex_pcc/src/lib.rs` | Retain typed PCC adapter; add operation/status/event support only behind contract tests. |
| `crates/cortex_rpc/src/lib.rs` | Reuse authenticated loopback/stream transport, version negotiation and actionable errors; add method support in service/controller rather than UI hacks. |
| `crates/cortex_conversation/` and Cortex service/controller | Reuse canonical persistence; create typed conversation list/select/send/stream bindings for GUI if missing. |
| `project.control.json`, `crates/cortex_project/src/lib.rs` | Reconcile legacy rollback values **with actual policy semantics** and typed validation. |
| `tools/control/tests/` and Rust unit/integration tests | Add shell layout, project switching, chat/console isolation, failed startup and operation correlation tests. |

**Proposed new modules** (only if source audit finds no equivalent): `PCCShellLayout.py` (shared shell/responsive layout tokens), `PCCCortexChatView.py` (transport-backed rendering), `PCCHealthPanel.py` (typed health state), `PCCViewState.py` (persistent non-source UI state). Reuse existing equivalents wherever possible. These are intended responsibilities, not mandatory extra files.

## 11. Implementation sequence and acceptance gates

### N1 — startup truth and schema compatibility (correctness prerequisite)
Inspect the exact manifest/Rust policy handling; resolve semantic rollback compatibility, typed contract validation, launch handshake and early-exit stderr. Acceptance: the known `git_or_snapshot` fixture fails preflight with actionable error or is safely migrated with equivalent semantics; the live application does not report Runtime Ready on early exit; successful startup actually reaches GUI-ready.

### N2 — shell and Projects normalization
Extract global shell/nav/action widgets; add Chat destination with real navigation; move Remove Registration far right; keep project registry selection and state. Acceptance: no duplicated nav bars, no project deletion from deregistration, all buttons work, keyboard navigation and different DPI pass.

### N3 — four-pane Workspace normalization
Replace Dashboard content with a real Chat container; keep separate Console; move health from header to pane four; make nav actions contextual; implement sash persistence/collapse policy. Acceptance: 10/40/40/10 default at desktop size, independent scrolling, no jitter at rapid resize, no loss of message draft or log buffer when switching global pages.

### N4 — shared conversations / global Chat home
Bind both Chat surfaces to existing persistent conversation IDs via service and streaming API, with project binding and recovery. Acceptance: send in Workspace, open same thread in Chat home, reply there, return to Workspace and observe exact persisted history without duplication; missing project cannot redirect to a namesake.

### N5 — operational chat integration
Registered PCC command catalog becomes actions Cortex can propose under existing permissions, with visible approval/diff/review, streaming results tied to operation IDs. Acceptance: chat-initiated build uses PCC once, shows stdout in Console and concise correlated status in Chat; cancel/retry and rollback are traceable; no source mutation without required authority.

### N6 — regression / release certification
Run Python self-test suite as part of actual Full Gate (not only independently); Rust fmt/check/test/clippy/build plus nested Forge if required; Windows GUI smoke at 100/125/150/200% scaling and small/normal/ultrawide widths; project switch, detached/closed Cortex service, model offline, source change invalidates GREEN, patch rollback, Git guarded commit. Capture signed/hash-verified evidence and clearly separate Source GREEN from GUI Runtime GREEN, Provider E2E and Release GREEN.

## 12. Non-goals and guardrails

- Do not replace the working PCC authority with unverified Rust Forge; ForgePY remains the universal front end pending explicit parity/takeover certification.
- Do not rebuild the existing project registry, Vault catalog or live console merely to achieve the new layout.
- Do not create an extra AI instance/chat database for each project, GUI or repository.
- Do not treat source compilation, successful process spawn or green status text as proof of a functional GUI.
- Do not automatically patch, commit, push or delete source without required approval and verification.
- This specification is **not** an installed patch. All N1–N6 milestones remain to be implemented and locally certified against the current working source.
