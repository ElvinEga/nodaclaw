# Nodamem Integration Notes

## Current Call Sites

Nodaclaw now opens a local Nodamem adapter from `crates/gateway/src/server.rs` and exposes it through gateway runtime state. The main integration points are:

- `crates/chat/src/lib.rs`: prompt/context assembly now calls Nodamem `recall_context`, keeps `## Verified Memory Context` unchanged, and conditionally appends a separate hypothetical planning block when the runtime policy detects planning or future-oriented reasoning.
- `crates/chat/src/lib.rs`: after a meaningful assistant response, Nodamem `propose_memory` is called with a validated exchange payload.
- `crates/chat/src/lib.rs`: after turn completion, Nodamem `record_outcome` is called for success/failure feedback, and any injected imagined scenarios are reviewed as accepted hypotheses or rejected scenarios.
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

## Manual Verification

Run the gateway with debug logging enabled and watch for the compact Nodamem trace events:

- Start the app with `RUST_LOG=moltis_chat=debug,moltis_nodamem_adapter=debug cargo run -p moltis-gateway`.
- Send a message with durable content such as a preference or project fact, then confirm logs show `recall_context`, `propose_memory`, and `record_outcome`.
- Send a follow-up question that should reuse that fact and confirm logs show either `nodamem context injected into prompt` or an explicit fallback message if Nodamem had no usable context.

## Write-Path Verification

- To verify write quality manually, submit the same durable fact twice and confirm admission logs show a duplicate-oriented action such as `merge_into_existing_node` or `reject` instead of another durable write.
- To verify contradiction handling, first store a preference like "the user prefers verbose release notes" and then send "the user no longer prefers verbose release notes"; the adapter should log a supersession decision and archive the older preference node.
- To verify lesson refinement, send one strategic lesson and then a more specific version of the same guidance; the lesson service should log `refined` or `weakened` outcomes while keeping evidence and provenance attached to the updated lesson record.
- To verify personality evolution, generate several validated outcomes with similar success or failure patterns and confirm `trait reinforcement recorded` or `trait weakening recorded` logs appear; if stable lessons already exist, `self-model refresh updated` should follow and the adapter will persist both `trait_events` and a new `self_model_snapshots` row.

## Prompt Formatting Rules

- Prompt injection is formatted in `crates/nodamem-adapter/src/lib.rs` as a bounded `Verified Memory Context` section.
- The formatter prefers concise summary text, validated lessons, and preference or goal memories before any general context.
- Duplicate lines are removed before prompt injection, and hypothetical or imagined wording is filtered so it is not presented as verified memory.
- The checkpoint summary is optional, and total prompt memory length is capped through `PromptMemoryFormatConfig` to keep the injected section compact.
- Hypothetical planning support is formatted separately as `## Hypothetical Planning Scenarios`; it is only added when the chat runtime policy detects planning, brainstorming, or future-oriented requests.
- The hypothetical block includes a short warning that scenarios are hypotheses, not facts, plus a compact strategy-continuity line derived from the self-model without dumping raw IDs, version fields, or full internal lists.

## Imagination Notes

- Grounded imagination now uses connected verified nodes, validated lessons, active goals or preferences, the current trait snapshot, and the latest self-model snapshot to build simulated scenarios.
- Imagined scenarios are stored only in `imagined_nodes` and carried through `MemoryPacket.imagined_scenarios`; they are not promoted into verified `nodes` without a separate validation flow.
- Manual verification: ask a planning-oriented question such as "brainstorm rollout options for next week" and confirm the system prompt contains both `## Verified Memory Context` and `## Hypothetical Planning Scenarios`, while a factual recall question should only include the verified block.
- After a planning turn succeeds or fails, confirm logs show `nodamem imagined scenario review requested` followed by `nodamem imagined scenario review completed`, and inspect `imagined_nodes.status` to verify scenarios were marked `accepted_as_hypothesis` or `rejected`.

## Inspection And Evaluation

- `crates/nodamem-adapter/src/lib.rs` now exposes compact developer-facing inspection views through `inspect_memory_flow(...)` for verified packet contents, hypothetical scenarios, self-model continuity sources, recent trait-update reasons, lesson audit reasons, and superseded preference or goal history.
- The same adapter crate also provides `run_evaluation_harness_at(...)`, which runs a disposable repeatable evaluation over stable preference recall, contradiction handling, duplicate suppression, grounded imagination, and scenario review feedback.
- Both outputs are intended for debugging and regression checks only; they do not change prompt assembly or verified-memory formatting.

## Graph UI Usage

- Open `Settings -> Nodamem Graph` to inspect the live graph snapshot exposed by the adapter-backed debug endpoints.
- Use search first when the graph is crowded; it matches node title, type, summary text, and raw ids.
- After selecting a node, enable `Neighborhood of selected node` and use `Focus selected` to reduce visual noise around the local cluster.
- Enable `Hide weak low-confidence nodes` when you want to inspect durable graph state instead of low-value candidates.
- Verified memory stays green or blue, imagined scenarios stay amber, and archived or superseded records remain muted with dashed outlines.
- Use the cluster/topic selector or the cluster chips to isolate one connected topic at a time, then use `Full graph` to return to the complete snapshot.
- Hover a node to get a compact preview of title, type, summary, scores, and status; hover an edge or select a node to reveal edge labels without permanently cluttering the graph.
- Selecting a verified node opens node actions for provenance, lesson links, supersession history, and same-cluster neighbor jumps.
- Provenance stays compact: it uses the existing inspection detail for source event ids, related nodes, audit reasons, lesson links, and any directly linked trait influence; imagined scenarios keep their own separate hypothetical view.
- Eligible verified nodes can now be archived from the selected-item card. The action records an append-only archive event with the provided reason, leaves the node inspectable in the graph, and keeps archived preference or goal nodes visible in history views.
