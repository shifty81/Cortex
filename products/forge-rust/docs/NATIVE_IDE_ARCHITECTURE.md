# Forge Native IDE Architecture

## Locked rule

Forge IDE is native Rust. No Chromium, Electron, WebView2, HTML/CSS/JavaScript or Monaco runtime is required.

## Layers

```text
Forge IDE tab (egui native)
  -> forge-ide
       -> editor session / tabs / search / guarded save
       -> language/terminal adapter discovery
       -> forge-protocol
            -> native stdio Content-Length JSON-RPC
            -> LSP / DAP compatible transport
  -> Forge project authority
       -> project contract/provider
       -> durable operations
       -> GitHub / Internal Git
       -> build/test/run
       -> Cortex
```

## Current editor model

The candidate intentionally starts with a bounded in-memory UTF-8 text model to minimize dependency and compile risk during takeover. It already provides multi-tab state, dirty/conflict truth, undo/redo and preimage-safe saves.

After the native candidate is GREEN, the editor core can be upgraded toward rope/piece-table storage and incremental tree parsing without changing the host architecture.

## Language intelligence

`forge-protocol` implements native stdio JSON-RPC framing used by LSP and DAP families. Language-tool discovery is separate from availability: a configured/suggested adapter is not marked operational merely because its name is known.

The next IDE hardening phase should add:

- asynchronous protocol reader/event queue;
- initialize/shutdown lifecycle;
- textDocument didOpen/didChange/didSave;
- diagnostics/completion/hover/symbol/rename/code-action models;
- DAP launch/attach/breakpoint/stack/variables models;
- Forge durable process/terminal integration.

## Authority

The IDE does not own a competing Git, project registry, build system or Cortex runtime. It consumes Forge authority and Cortex intelligence.
