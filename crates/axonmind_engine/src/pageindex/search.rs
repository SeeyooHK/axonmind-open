// Reasoning-based ranking/enrichment adapted from rusty-pageindex (MIT) and the PageIndex pattern
// (VectifyAI/PageIndex, MIT). Reimplemented over axonmind's NormalizedDocument and store; the
// retrieval funnel (BM25 shortlist → LLM rerank) is original.

use axonmind_core::AxonMindError;

use crate::extract::llm::LlmProvider;
use crate::query::reasoning::{ReasoningSearchInput, ReasoningSearchOutput, RetrievedSection};

use super::{PageIndexSearchCfg, store::PageIndexStore};

const STAGE2_SYSTEM: &str =
    "You are a document retrieval assistant. Given a search query and a numbered list of document \
     sections, return the numbers of the most relevant sections in order from most to least \
     relevant, best first. Reply with just the numbers separated by commas (e.g. '3, 1, 2'). \
     If no sections are relevant, reply with exactly 'NONE'.";

/// Two-stage retrieval funnel.
///
/// Stage 1 (always): BM25 over page_section_fts → bounded shortlist.
/// Stage 2 (when llm is Some): LLM judges/reranks the shortlist. Skipped gracefully otherwise.
pub async fn reasoning_search(
    input: ReasoningSearchInput,
    store: &PageIndexStore,
    llm: Option<&dyn LlmProvider>,
    cfg: &PageIndexSearchCfg,
) -> Result<ReasoningSearchOutput, AxonMindError> {
    // ── Stage 1: BM25 recall ────────────────────────────────────────────────────
    let fts_query = sanitize_fts_query(&input.query);
    if fts_query.is_empty() {
        return Ok(ReasoningSearchOutput {
            sections: vec![],
            reasoning_applied: false,
        });
    }

    let candidate_ids = store
        .bm25_shortlist(&fts_query, cfg.shortlist_limit)
        .await?;
    if candidate_ids.is_empty() {
        return Ok(ReasoningSearchOutput {
            sections: vec![],
            reasoning_applied: false,
        });
    }

    let mut rows = store.fetch_sections(&candidate_ids).await?;

    // Preserve BM25 rank order (fetch_sections returns rows in arbitrary order).
    let id_to_rank: std::collections::HashMap<&str, usize> = candidate_ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    rows.sort_by_key(|r| id_to_rank.get(r.section_id.as_str()).copied().unwrap_or(usize::MAX));

    // Optional: filter to a single document.
    if let Some(ref doc_id) = input.doc_node_id {
        rows.retain(|r| &r.doc_node_id == doc_id);
    }

    let max_results = input.max_results.unwrap_or(cfg.max_results);

    // ── Stage 2: LLM reasoning precision ───────────────────────────────────────
    let reasoning_applied;
    let final_rows = if let Some(llm) = llm {
        reasoning_applied = true;
        rerank_with_llm(&input.query, rows, llm, max_results).await?
    } else {
        reasoning_applied = false;
        rows.into_iter().take(max_results).collect()
    };

    let sections = final_rows
        .into_iter()
        .map(|row| RetrievedSection {
            doc_node_id: row.doc_node_id,
            section_id: row.section_id,
            title: row.title,
            text: row.text.unwrap_or_default(),
            span_start: row.span_start as usize,
            span_end: row.span_end as usize,
            path: row
                .path
                .split(" \u{203a} ")
                .map(|s| s.to_string())
                .collect(),
        })
        .collect();

    Ok(ReasoningSearchOutput {
        sections,
        reasoning_applied,
    })
}

async fn rerank_with_llm(
    query: &str,
    rows: Vec<super::tree::SectionRow>,
    llm: &dyn LlmProvider,
    max_results: usize,
) -> Result<Vec<super::tree::SectionRow>, AxonMindError> {
    if rows.is_empty() {
        return Ok(vec![]);
    }

    // Build numbered candidate list: "N. Title — path — preview"
    let numbered: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let preview = row
                .summary
                .as_deref()
                .or(row.text.as_deref())
                .map(|t| {
                    let s: String = t.chars().take(150).collect();
                    s
                })
                .unwrap_or_default();
            format!("{}. {} — {} — {}", i + 1, row.title, row.path, preview)
        })
        .collect();

    let user = format!(
        "Query: {query}\n\nSections:\n{}",
        numbered.join("\n")
    );

    let response = llm.complete(STAGE2_SYSTEM, &user).await?;
    let response = response.trim();

    if response.eq_ignore_ascii_case("NONE") {
        return Ok(vec![]);
    }

    // Parse integers from the response (split on any non-digit sequence).
    let selected_indices: Vec<usize> = response
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|s| s.parse::<usize>().ok())
        .filter(|&n| n >= 1 && n <= rows.len())
        .map(|n| n - 1)
        .collect();

    if selected_indices.is_empty() {
        // Fallback: return BM25 order if LLM response is unparseable.
        return Ok(rows.into_iter().take(max_results).collect());
    }

    // Deduplicate while preserving LLM rank order.
    let mut seen = std::collections::HashSet::new();
    let reranked: Vec<_> = selected_indices
        .into_iter()
        .filter(|&i| seen.insert(i))
        .take(max_results)
        .map(|i| rows[i].clone())
        .collect();

    Ok(reranked)
}

/// Sanitize a query string for FTS5 MATCH: wrap each word in double-quotes so
/// special characters are treated as literals, not FTS5 operators.
fn sanitize_fts_query(q: &str) -> String {
    q.split_whitespace()
        .map(|w| format!("\"{}\"", w.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" ")
}
