use super::llm::{
    EntityExtractionInput, EntityExtractionOutput, LlmProvider, RelationExtractionInput,
    RelationExtractionOutput, SemanticLink, SemanticLinkInput, SemanticLinkOutput,
    extract_json_object,
};
use async_trait::async_trait;
use axonmind_core::AxonMindError;
use seeyoo_llm::api_mod::{ApiProvider, MessageBlock, ProviderMessage};
use seeyoo_llm::types::ToolDefinition;
use serde::Deserialize;
use std::sync::Arc;

/// Render a concept list as "0. Foo\n1. Bar" so the LLM can refer to concepts by index.
fn numbered(items: &[String]) -> String {
    items
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{i}. {s}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Adapter implementing axonmind's high-level `LlmProvider` trait via any seeyoo_llm provider.
///
/// Use `AxonMindEngine::set_llm_provider(Arc::new(SeeyooAdapter::new(...)))`.
pub struct SeeyooAdapter {
    provider: Arc<dyn ApiProvider + Send + Sync>,
    api_key: String,
    model: Option<String>,
}

impl SeeyooAdapter {
    pub fn new(
        provider: Arc<dyn ApiProvider + Send + Sync>,
        api_key: impl Into<String>,
        model: Option<String>,
    ) -> Self {
        Self {
            provider,
            api_key: api_key.into(),
            model,
        }
    }
}

async fn complete_json(
    provider: &dyn ApiProvider,
    api_key: &str,
    model: Option<&str>,
    system: &str,
    user: &str,
) -> Result<String, AxonMindError> {
    let messages = vec![ProviderMessage::user(user.to_string())];
    let text = provider
        .complete(
            system,
            messages,
            Vec::<ToolDefinition>::new(),
            api_key,
            model,
        )
        .await
        .map_err(|e| AxonMindError::LlmProvider(e.to_string()))?;
    Ok(text)
}

#[async_trait]
impl LlmProvider for SeeyooAdapter {
    async fn complete(&self, system: &str, user: &str) -> Result<String, AxonMindError> {
        let raw = complete_json(
            self.provider.as_ref(),
            &self.api_key,
            self.model.as_deref(),
            system,
            user,
        )
        .await?;
        Ok(extract_json_object(&raw).to_string())
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
             Return only valid JSON (no markdown fences): {{\"entities\": [[\"Kind\", \"Name\", \"verbatim_quote\"], ...]}}\n\
             Kind must be one of: Kpi, Metric, Objective, Initiative, Risk, Opportunity, \
             Decision, Insight, Person, Team, Customer, Function, Product, Market, Process, System, Action.\n\
             Avoid duplicating these already-known entities: {existing}."
        );

        let raw = complete_json(
            self.provider.as_ref(),
            &self.api_key,
            self.model.as_deref(),
            &system,
            &input.document_text,
        )
        .await?;

        if raw.trim().is_empty() {
            return Ok(EntityExtractionOutput { entities: vec![] });
        }
        #[derive(Deserialize)]
        struct Resp {
            entities: Vec<(String, String, String)>,
        }
        let parsed: Resp = serde_json::from_str(extract_json_object(&raw))
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
             Return only valid JSON (no markdown fences): \
             {\"edge_kind\": \"...\", \"confidence\": 0.0–1.0, \"quote\": \"verbatim_text\"}\n\
             edge_kind must be one of: Influences, Causes, CorrelatesWith, DependsOn, Blocks, \
             DerivedFrom, Improves, Degrades, OwnedBy, MeasuredBy, MentionedIn, DecidedBy, AssignedTo, \
             InFunction, ForProduct, Impacts, NextAction.";

        let user = format!(
            "Entity A: {}\nEntity B: {}\nContext: {}",
            input.entity_a, input.entity_b, input.context_paragraph
        );

        let raw = complete_json(
            self.provider.as_ref(),
            &self.api_key,
            self.model.as_deref(),
            system,
            &user,
        )
        .await?;

        #[derive(Deserialize)]
        struct Resp {
            edge_kind: String,
            confidence: f32,
            quote: String,
        }
        let parsed: Resp = serde_json::from_str(extract_json_object(&raw))
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
             Return only valid JSON (no markdown fences): {\"links\": [{\"from_new\": <int>, \
             \"to_existing\": <int>, \"edge_kind\": \"...\", \"confidence\": 0.0–1.0, \
             \"rationale\": \"short reason\"}, ...]}\n\
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

        let raw = complete_json(
            self.provider.as_ref(),
            &self.api_key,
            self.model.as_deref(),
            system,
            &user,
        )
        .await?;

        #[derive(Deserialize)]
        struct Resp {
            links: Vec<SemanticLink>,
        }
        let parsed: Resp = serde_json::from_str(extract_json_object(&raw))
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
        match self.provider.id() {
            seeyoo_llm::types::LlmProvider::Local | seeyoo_llm::types::LlmProvider::Ollama => {
                return Err(AxonMindError::LlmProvider(format!(
                    "{} does not support image OCR through this provider path",
                    self.provider.display_name()
                )));
            }
            _ => {}
        }

        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let data_base64 = STANDARD.encode(bytes);
        let messages = vec![ProviderMessage::User(vec![
            MessageBlock::Image {
                data_base64,
                mime_type: mime_type.to_string(),
            },
            MessageBlock::Text {
                text: "You're a skilled image content extractor. Extract the rich text content \
                       from the image in structured markdown format. Preserve headings, lists, \
                       tables, footers, small print, internal charts/graphs/images, bold and \
                       italic text, and paragraphs. If there are tables, extract them as markdown \
                       tables. Do not repeat the same information multiple times. Follow standard \
                       markdown format and do not translate special characters. Do not add \
                       commentary outside the markdown structure. Review the extraction carefully \
                       and ensure it accurately represents the image content."
                    .to_string(),
            },
        ])];
        self.provider
            .complete(
                "You are a document transcriber. Extract all text and describe visual content as markdown.",
                messages,
                Vec::<seeyoo_llm::types::ToolDefinition>::new(),
                &self.api_key,
                self.model.as_deref(),
            )
            .await
            .map_err(|e| AxonMindError::LlmProvider(e.to_string()))
    }

    async fn explain_kpi_rationale(
        &self,
        kpi_name: &str,
        evidence_quotes: &[String],
    ) -> Result<String, AxonMindError> {
        let system = "You are a business analyst. Write a concise rationale (2–4 sentences) \
             explaining a KPI's significance and current state based on the evidence.\n\
             Return only valid JSON (no markdown fences): {\"rationale\": \"...\"}";

        let numbered = evidence_quotes
            .iter()
            .enumerate()
            .map(|(i, q)| format!("{}. {q}", i + 1))
            .collect::<Vec<_>>()
            .join("\n");

        let user = format!("KPI: {kpi_name}\n\nEvidence:\n{numbered}");

        let raw = complete_json(
            self.provider.as_ref(),
            &self.api_key,
            self.model.as_deref(),
            system,
            &user,
        )
        .await?;

        #[derive(Deserialize)]
        struct Resp {
            rationale: String,
        }
        let parsed: Resp = serde_json::from_str(extract_json_object(&raw))
            .map_err(|e| AxonMindError::LlmProvider(format!("rationale parse: {e}")))?;

        Ok(parsed.rationale)
    }
}
