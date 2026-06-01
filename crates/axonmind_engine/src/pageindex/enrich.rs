// Reasoning-based ranking/enrichment adapted from rusty-pageindex (MIT) and the PageIndex pattern
// (VectifyAI/PageIndex, MIT). Reimplemented over axonmind's NormalizedDocument and store; the
// retrieval funnel (BM25 shortlist → LLM rerank) is original.

use axonmind_core::AxonMindError;

use crate::extract::llm::LlmProvider;

use super::tree::PageSection;

const ENRICH_SYSTEM: &str =
    "Summarize the following document section in 2-3 sentences, capturing the key business \
     information. Focus on quantitative facts and business metrics when present. Be concise.";

/// Bottom-up LLM enrichment: fill `PageSection.summary` for every section.
/// Processes children before parents (bottom-up); siblings at each level run as a
/// parallel batch (chunked by `max_concurrency`).
/// Returns an overall doc-level summary constructed from root summaries, or None on empty roots.
pub async fn enrich_tree(
    roots: &mut [PageSection],
    llm: &dyn LlmProvider,
    max_concurrency: usize,
) -> Result<Option<String>, AxonMindError> {
    enrich_siblings(roots, llm, max_concurrency).await
}

// Recursive bottom-up enrichment. Uses Box::pin to allow async recursion.
fn enrich_siblings<'a>(
    sections: &'a mut [PageSection],
    llm: &'a dyn LlmProvider,
    max_concurrency: usize,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Option<String>, AxonMindError>> + Send + 'a>,
> {
    Box::pin(async move {
        // Bottom-up: enrich children of each section before building the parent's prompt.
        for section in sections.iter_mut() {
            enrich_siblings(&mut section.children, llm, max_concurrency).await?;
        }

        // Build prompts (children are now enriched, so their summaries appear in the prompt).
        let prompts: Vec<String> = sections.iter().map(build_enrich_prompt).collect();

        // Run prompts in parallel chunks bounded by max_concurrency.
        let mut all_summaries: Vec<Option<String>> = Vec::with_capacity(prompts.len());
        for chunk in prompts.chunks(max_concurrency.max(1)) {
            let futures: Vec<_> = chunk
                .iter()
                .map(|user| llm.complete(ENRICH_SYSTEM, user))
                .collect();
            let results = futures_util::future::join_all(futures).await;
            for result in results {
                match result {
                    Ok(s) => all_summaries.push(Some(s.trim().to_string())),
                    Err(_) => all_summaries.push(None),
                }
            }
        }

        // Apply summaries.
        for (section, summary) in sections.iter_mut().zip(all_summaries.iter()) {
            if let Some(s) = summary {
                section.summary = Some(s.clone());
            }
        }

        Ok(sections.first().and_then(|s| s.summary.clone()))
    })
}

fn build_enrich_prompt(section: &PageSection) -> String {
    let text = section.text.as_deref().unwrap_or("(no body text)");
    let child_summaries: Vec<String> = section
        .children
        .iter()
        .filter_map(|c| {
            c.summary
                .as_ref()
                .map(|s| format!("- {} (subsection): {}", c.title, s))
        })
        .collect();

    let mut user = format!("Section title: {}\n\nContent:\n{}", section.title, text);
    if !child_summaries.is_empty() {
        user.push_str("\n\nChild subsection summaries:\n");
        user.push_str(&child_summaries.join("\n"));
    }
    user
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OrderTrackingProvider {
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl OrderTrackingProvider {
        fn new() -> Self {
            Self {
                calls: std::sync::Mutex::new(vec![]),
            }
        }

        fn call_order(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for OrderTrackingProvider {
        async fn complete(&self, _system: &str, user: &str) -> Result<String, AxonMindError> {
            let title = user
                .lines()
                .next()
                .unwrap_or("")
                .trim_start_matches("Section title: ")
                .to_string();
            self.calls.lock().unwrap().push(title.clone());
            Ok(format!("Summary of {title}"))
        }

        async fn extract_entities(
            &self,
            _input: crate::extract::llm::EntityExtractionInput,
        ) -> Result<crate::extract::llm::EntityExtractionOutput, AxonMindError> {
            Ok(crate::extract::llm::EntityExtractionOutput { entities: vec![] })
        }

        async fn extract_relations(
            &self,
            _input: crate::extract::llm::RelationExtractionInput,
        ) -> Result<crate::extract::llm::RelationExtractionOutput, AxonMindError> {
            Err(AxonMindError::Ingest {
                message: "not implemented".into(),
            })
        }

        async fn link_concepts(
            &self,
            _input: crate::extract::llm::SemanticLinkInput,
        ) -> Result<crate::extract::llm::SemanticLinkOutput, AxonMindError> {
            Ok(crate::extract::llm::SemanticLinkOutput { links: vec![] })
        }

        async fn explain_kpi_rationale(
            &self,
            _kpi_name: &str,
            _evidence_quotes: &[String],
        ) -> Result<String, AxonMindError> {
            Ok(String::new())
        }
    }

    #[tokio::test]
    async fn test_enrich_bottom_up_order() {
        // Child must be enriched before parent (bottom-up order).
        // Tree: Root → [Leaf]
        let mut root = PageSection {
            id: "root".to_string(),
            title: "Root".to_string(),
            level: 1,
            summary: None,
            text: Some("root text".to_string()),
            span_start: 0,
            span_end: 100,
            children: vec![PageSection {
                id: "leaf".to_string(),
                title: "Leaf".to_string(),
                level: 2,
                summary: None,
                text: Some("leaf text".to_string()),
                span_start: 10,
                span_end: 50,
                children: vec![],
            }],
        };

        let provider = OrderTrackingProvider::new();
        let mut roots = vec![root.clone()];
        enrich_tree(&mut roots, &provider, 4)
            .await
            .expect("enrich failed");

        let order = provider.call_order();
        // "Leaf" must appear before "Root" in the call order (bottom-up).
        let leaf_pos = order.iter().position(|s| s == "Leaf").unwrap();
        let root_pos = order.iter().position(|s| s == "Root").unwrap();
        assert!(
            leaf_pos < root_pos,
            "Leaf should be enriched before Root (bottom-up), got order: {order:?}"
        );

        // Root's summary should contain "Root" and leaf summary should be set.
        assert!(
            roots[0].summary.is_some(),
            "root should have a summary after enrichment"
        );
        assert!(
            roots[0].children[0].summary.is_some(),
            "leaf should have a summary after enrichment"
        );
    }
}
