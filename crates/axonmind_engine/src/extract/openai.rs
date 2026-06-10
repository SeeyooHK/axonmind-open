use super::llm::{
    EntityExtractionInput, EntityExtractionOutput, LlmProvider, RelationExtractionInput,
    RelationExtractionOutput, SemanticLink, SemanticLinkInput, SemanticLinkOutput,
    extract_json_object,
};
use async_trait::async_trait;
use axonmind_core::AxonMindError;
use serde::Deserialize;

/// Render a concept list as "0. Foo\n1. Bar" so the LLM can refer to concepts by index.
fn numbered(items: &[String]) -> String {
    items
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{i}. {s}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// OpenAI-compatible LLM provider. Works with any API that speaks the OpenAI chat format.
///
/// Usage:
///   let provider = OpenAiProvider::new("https://api.openai.com/v1", api_key, "gpt-4o-mini");
///   engine.set_llm_provider(Arc::new(provider));
pub struct OpenAiProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl OpenAiProvider {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    async fn complete_json(&self, system: &str, user: &str) -> Result<String, AxonMindError> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.model,
            "response_format": {"type": "json_object"},
            "messages": [
                {"role": "system", "content": system},
                {"role": "user",   "content": user}
            ]
        });

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| AxonMindError::LlmProvider(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(AxonMindError::LlmProvider(format!("HTTP {status}: {text}")));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AxonMindError::LlmProvider(e.to_string()))?;

        json["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| AxonMindError::LlmProvider("no content in LLM response".into()))
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn complete(&self, system: &str, user: &str) -> Result<String, AxonMindError> {
        // `complete_json` requests `response_format: json_object`, so the content is already
        // clean JSON with no markdown fences.
        self.complete_json(system, user).await
    }

    async fn extract_entities(
        &self,
        input: EntityExtractionInput,
    ) -> Result<EntityExtractionOutput, AxonMindError> {
        let existing = if input.existing_node_names.is_empty() {
            "none".to_string()
        } else {
            input.existing_node_names.join(", ")
        };

        let system = format!(
            "You are a business knowledge graph entity extractor.\n\
             Extract the underlying business CONCEPTS from the document — not its structure.\n\
             Name rules:\n\
             - Name the concept, never a section heading. Strip any leading clause or section \
             number: \"22.3 Platform Warranties\" -> \"Platform Warranties\", \
             \"14.7 Data Retention\" -> \"Data Retention\".\n\
             - Use a short canonical noun phrase. If several headings describe the same concept, \
             return it once. Reuse an existing name verbatim when it refers to the same concept.\n\
             Skip purely structural headings that carry no business meaning (e.g. Definitions, \
             Background, Notices, Signature, Schedule N, General Provisions, Table of Contents).\n\
             Return JSON: {{\"entities\": [[\"Kind\", \"Name\", \"verbatim_quote\"], ...]}}\n\
             Kind must be one of: Kpi, Metric, Objective, Initiative, Risk, Opportunity, \
             Decision, Insight, Person, Team, Customer, Function, Product, Market, Process, System, Action.\n\
             Avoid duplicating these already-known entities: {existing}."
        );

        let content = self.complete_json(&system, &input.document_text).await?;
        if content.trim().is_empty() {
            return Ok(EntityExtractionOutput { entities: vec![] });
        }

        #[derive(Deserialize)]
        struct Resp {
            entities: Vec<(String, String, String)>,
        }
        let parsed: Resp = serde_json::from_str(extract_json_object(&content))
            .map_err(|e| AxonMindError::LlmProvider(format!("entity parse: {e}")))?;

        Ok(EntityExtractionOutput {
            entities: parsed.entities,
        })
    }

    async fn extract_relations(
        &self,
        input: RelationExtractionInput,
    ) -> Result<RelationExtractionOutput, AxonMindError> {
        let system = "You are a business knowledge graph relation extractor.\n\
             Determine the relationship between two entities in the given context.\n\
             Return JSON: {\"edge_kind\": \"...\", \"confidence\": 0.0–1.0, \"quote\": \"verbatim_text\"}\n\
             edge_kind must be one of: Influences, Causes, CorrelatesWith, DependsOn, Blocks, \
             DerivedFrom, Improves, Degrades, OwnedBy, MeasuredBy, MentionedIn, DecidedBy, AssignedTo, \
             InFunction, ForProduct, Impacts, NextAction.";

        let user = format!(
            "Entity A: {}\nEntity B: {}\nContext: {}",
            input.entity_a, input.entity_b, input.context_paragraph
        );

        let content = self.complete_json(system, &user).await?;

        #[derive(Deserialize)]
        struct Resp {
            edge_kind: String,
            confidence: f32,
            quote: String,
        }
        let parsed: Resp = serde_json::from_str(extract_json_object(&content))
            .map_err(|e| AxonMindError::LlmProvider(format!("relation parse: {e}")))?;

        Ok(RelationExtractionOutput {
            edge_kind: parsed.edge_kind,
            confidence: parsed.confidence.clamp(0.0, 1.0),
            quote: parsed.quote,
        })
    }

    async fn link_concepts(
        &self,
        input: SemanticLinkInput,
    ) -> Result<SemanticLinkOutput, AxonMindError> {
        let system = "You are a business knowledge graph cross-document linker.\n\
             You are given concepts from a NEW document and concepts ALREADY in the graph (from \
             other documents). Identify only STRONG, SPECIFIC business relationships between a new \
             concept and an existing concept. Skip generic or weak associations — returning an \
             empty list is correct when nothing strongly relates.\n\
             Return JSON: {\"links\": [{\"from_new\": <int>, \"to_existing\": <int>, \
             \"edge_kind\": \"...\", \"confidence\": 0.0–1.0, \"rationale\": \"short reason\"}, ...]}\n\
             from_new indexes the New concepts list; to_existing indexes the Existing concepts list.\n\
             edge_kind must be one of: Influences, Causes, DependsOn, DerivedFrom, Blocks, Improves, Degrades, \
             Impacts, Contradicts, Corroborates.\n\
             Use Blocks when an existing constraint limits or governs a new capability; DependsOn \
             when one concept requires the other.";

        let user = format!(
            "New concepts:\n{}\n\nExisting concepts:\n{}",
            numbered(&input.new_concepts),
            numbered(&input.existing_concepts),
        );

        let content = self.complete_json(system, &user).await?;

        #[derive(Deserialize)]
        struct Resp {
            links: Vec<SemanticLink>,
        }
        let parsed: Resp = serde_json::from_str(extract_json_object(&content))
            .map_err(|e| AxonMindError::LlmProvider(format!("semantic link parse: {e}")))?;

        Ok(SemanticLinkOutput {
            links: parsed.links,
        })
    }

    async fn transcribe_image(
        &self,
        bytes: &[u8],
        mime_type: &str,
    ) -> Result<String, AxonMindError> {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let data_url = format!("data:{};base64,{}", mime_type, STANDARD.encode(bytes));
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.model,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "image_url", "image_url": { "url": data_url } },
                    { "type": "text", "text": "Transcribe all visible text exactly as written. \
                       For diagrams, charts, or handwritten content, describe them clearly in \
                       markdown. Return only the transcribed content with no commentary." }
                ]
            }]
        });

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| AxonMindError::LlmProvider(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(AxonMindError::LlmProvider(format!("HTTP {status}: {text}")));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AxonMindError::LlmProvider(e.to_string()))?;

        json["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| AxonMindError::LlmProvider("no content in vision response".into()))
    }

    async fn explain_kpi_rationale(
        &self,
        kpi_name: &str,
        evidence_quotes: &[String],
    ) -> Result<String, AxonMindError> {
        let system = "You are a business analyst. Write a concise rationale (2–4 sentences) \
             explaining a KPI's significance and current state based on the evidence.\n\
             Return JSON: {\"rationale\": \"...\"}";

        let numbered = evidence_quotes
            .iter()
            .enumerate()
            .map(|(i, q)| format!("{}. {q}", i + 1))
            .collect::<Vec<_>>()
            .join("\n");

        let user = format!("KPI: {kpi_name}\n\nEvidence:\n{numbered}");
        let content = self.complete_json(system, &user).await?;

        #[derive(Deserialize)]
        struct Resp {
            rationale: String,
        }
        let parsed: Resp = serde_json::from_str(extract_json_object(&content))
            .map_err(|e| AxonMindError::LlmProvider(format!("rationale parse: {e}")))?;

        Ok(parsed.rationale)
    }
}
