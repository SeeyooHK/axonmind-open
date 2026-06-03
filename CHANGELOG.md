# Changelog

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
