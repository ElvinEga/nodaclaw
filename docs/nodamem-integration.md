# Nodamem Integration Notes

## Current Call Sites

Nodaclaw now opens a local Nodamem adapter from `crates/gateway/src/server.rs` and exposes it through gateway runtime state. The main integration points are:

- `crates/chat/src/lib.rs`: prompt/context assembly now calls Nodamem `recall_context` and appends a compact external-memory section to the system prompt.
- `crates/chat/src/lib.rs`: after a meaningful assistant response, Nodamem `propose_memory` is called with a validated exchange payload.
- `crates/chat/src/lib.rs`: after turn completion, Nodamem `record_outcome` is called for success/failure feedback.
- `crates/nodamem-adapter/src/lib.rs`: all Nodamem-specific loading, conversion, and persistence logic is isolated behind the adapter.

## Old Memory Path Still Active

This is a gradual integration. The existing Moltis/Nodaclaw file-backed memory path still handles:

- `memory_search`, `memory_get`, and `memory_save` tools
- `MEMORY.md` and `memory/*.md` indexing through `moltis-memory`
- silent memory compaction writes
- the built-in `session-memory` hook
- filesystem watcher re-indexing and legacy RAG search

Nodaclaw does not write directly into Nodamem tables from chat/gateway modules. All graph writes go through the adapter, which first calls Nodamem APIs and then persists only adapter-approved results.

## Known Gaps

- Explicit user acceptance/rejection UI feedback is not yet wired; current outcome recording covers turn success/failure.
- Legacy file memory and Nodamem graph memory currently coexist. Tool-based memory retrieval still uses the old file-backed search path.
- Lesson persistence remains conservative; the first pass focuses on read recall, validated memory proposals, and outcome-driven trait updates.
