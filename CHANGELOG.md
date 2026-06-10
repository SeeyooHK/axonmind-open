# Changelog

## PR #4 — Image Ingest, Vision OCR & Parsed Inspector

- **Image ingest support** — `jpg`, `jpeg`, `png`, `bmp`, `webp`, `tiff`, `tif`, and `gif` are now accepted by the engine and Tauri demo file picker/drop zone.
- **LLM-first image transcription with OCR fallback** — image files can be transcribed into structured markdown through the active LLM provider, then normalized through the existing markdown ingest path. Empty transcriptions now fail loudly or fall back to OCR instead of silently indexing blank content.
- **Provider-path fixes for image OCR** — the Codex session provider path now sends image attachments to `codex exec` and pipes the prompt through stdin, fixing the broken image OCR flow on that adapter.
- **JSON parse hardening for provider output** — LLM JSON parsing now strips markdown fences and provider preambles before deserialization, fixing extraction failures from providers that return valid JSON wrapped in extra text.
- **Inspect modal now shows parsed content for images too** — the file inspector now renders parsed markdown/text for processed binary documents and images instead of trying to UTF-8 decode the original binary file.
- **Cached preview reads for processed files** — inspect first reads stored pageindex sections for `doc.*` nodes, then falls back to a preview parse when needed. This avoids empty panels and repeated reparsing for already-indexed files.
- **Regenerate clears stale pageindex rows before rebuild** — pageindex sections for a document are removed before re-indexing so refreshed parses are reflected cleanly in Search Contents and inspect views.

---

## After PR #3 — Graph Diff & FK Cascade Fix (merged 2026-06-07)

- **Graph diff engine** (`crates/axonmind_engine/src/query/diff.rs`) — typed diff between two `GraphExportV1` snapshots; returns added, modified, and removed nodes and edges with a list of changed fields per entry, plus summary counts and a warnings list.
- **`graph_diff` and `graph_stats` engine methods** — `graph_diff(before, after)` computes the diff; `graph_stats()` returns per-kind node counts and total edge count. Both exposed as Tauri commands and MCP tools.
- **CLI subcommands** — `axonmind graph-diff <before.json> <after.json>` and `axonmind graph-stats --workspace <dir>` with `--json` flag support.
- **Graph Diff UI** — "Graph Diff" button on the Processed Files page captures a before/after snapshot on every Regenerate and opens a tabbed modal (Overview, Nodes, Edges, Warnings) with Copy Summary and Export JSON actions. A one-line diff toast appears after any ingest or bulk regenerate.
- **React hooks** — `useGraphDiff` and `useGraphStats` transport-agnostic hooks added; `graphDiff()` and `graphStats()` methods added to `AxonMindTransport` and the Tauri transport implementation.
- **TypeScript bindings** — `GraphDiff`, `NodeChange`, `EdgeChange`, `DiffCounts`, `DiffSection`, `GraphStatsOutput`, `NodeKindCount` generated from Rust types.
- **Bug fix — FK cascade** (`store/sqlite.rs`) — `PRAGMA foreign_keys = ON` was only applied to the migration connection, not every pooled connection. Added a `post_create` hook to enforce FK on each connection. Without this fix, `DeleteNode` did not cascade to edges, causing concept counts to grow on every Regenerate.
- **Orphaned edge cleanup** (`store/mod.rs`) — after `DeleteNode`, edges that lost all evidence via FK cascade are now explicitly removed and their FTS5 entries synced.
- **34 new tests** in `crates/axonmind_engine/tests/diff.rs` covering node add/remove/modify, edge add/remove/modify, no-change, and mixed-change cases.

---

## After PR #2 — MCP Service & File Inspector (merged 2026-06-02)

- **`axonmind mcp --workspace <dir>` server** — spec-compliant JSON-RPC 2.0 stdio MCP server exposing all 8 query tools (`focus_kpi`, `explain_kpi`, `get_evidence`, `impact_radius`, `trace_decision`, `suggest_actions`, `graph_search`, `reasoning_search`) with protocol version negotiation, initialization guard, and strict request validation; no new dependencies.
- **File List side-by-side inspector** — the Inspect button on the Processed Files page now opens the ORIGINAL / EXTRACTED modal; binary formats (pptx, docx, pdf, xlsx) show parsed text instead of a UTF-8 error.
- **18 new tests** — 10 CLI transport tests (JSON-RPC lifecycle, error codes, init guard) and 8 engine dispatch tests (schema drift guard covering all optional fields, unknown tool, happy-path).

---

## After PR #1 — Vectorless Retrieval (merged 2026-06-02)

- **Page index subsystem** (`axonmind_engine::pageindex`) — Vectorless, Reasoning-based Retrieval. document sections stored in a new FTS5 table with heading tree, breadcrumb paths, and optional LLM enrichment summaries.
- **`reasoning_search` query** — two-stage BM25 recall + LLM re-ranking over indexed document sections; degrades gracefully to BM25-only when no LLM provider is configured.
- **CLI `query reasoning-search`** — `axonmind query reasoning-search <query> [--doc <id>] [--limit N]` subcommand wired through to the new engine method.

---

## Before PR #1 — Initial Release

- **7-tool knowledge graph engine** — `focus_kpi`, `explain_kpi`, `get_evidence`, `impact_radius`, `trace_decision`, `suggest_actions`, and `graph_search` backed by a SQLite + FTS5 store with a petgraph in-memory cache.
- **Multi-format ingestion** — Markdown, PDF, DOCX, HTML, spreadsheet, and plain-text documents parsed and extracted into graph nodes, edges, and evidence with SHA-256 blob retention.
- **Tauri + CLI hosts** — workspace management, graph export/import, and LLM provider wiring (Anthropic, OpenAI, Gemini, Ollama) via `axonmind_tauri` and `axonmind_cli`.
