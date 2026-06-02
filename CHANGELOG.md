# Changelog

## After PR #1 — Vectorless Retrieval (merged 2026-06-02)

- **Page index subsystem** (`axonmind_engine::pageindex`) — Vectorless, Reasoning-based Retrieval. document sections stored in a new FTS5 table with heading tree, breadcrumb paths, and optional LLM enrichment summaries.
- **`reasoning_search` query** — two-stage BM25 recall + LLM re-ranking over indexed document sections; degrades gracefully to BM25-only when no LLM provider is configured.
- **CLI `query reasoning-search`** — `axonmind query reasoning-search <query> [--doc <id>] [--limit N]` subcommand wired through to the new engine method.

---

## Before PR #1 — Initial Release

- **7-tool knowledge graph engine** — `focus_kpi`, `explain_kpi`, `get_evidence`, `impact_radius`, `trace_decision`, `suggest_actions`, and `graph_search` backed by a SQLite + FTS5 store with a petgraph in-memory cache.
- **Multi-format ingestion** — Markdown, PDF, DOCX, HTML, spreadsheet, and plain-text documents parsed and extracted into graph nodes, edges, and evidence with SHA-256 blob retention.
- **Tauri + CLI hosts** — workspace management, graph export/import, and LLM provider wiring (Anthropic, OpenAI, Gemini, Ollama) via `axonmind_tauri` and `axonmind_cli`.
