/// Phase 3: Pluggable LLM provider trait.
///
/// Contract:
/// - No raw free-form LLM output may enter `GraphMutation`. All output must pass
///   JSON schema validation before being converted to mutations.
/// - Every LLM-created edge requires at least one quote-backed `Evidence` record.
/// - Default confidence for LLM-extracted entities/edges: `Confidence::LLM` (0.50).
/// - `is_tainted = true` on all LLM-created nodes/edges until corroborated by clean evidence.
///
/// The built-in `reqwest`-based OpenAI-compatible provider is behind the `llm` feature flag.
/// Hosts may inject their own provider via `AxonMindEngine::set_llm_provider`.
use async_trait::async_trait;
use axonmind_core::AxonMindError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityExtractionInput {
    pub document_text: String,
    pub existing_node_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityExtractionOutput {
    /// Each tuple: (NodeKind as string, name, quote from text).
    pub entities: Vec<(String, String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationExtractionInput {
    pub entity_a: String,
    pub entity_b: String,
    pub context_paragraph: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationExtractionOutput {
    /// EdgeKind as string, validated against the enum before mutation.
    pub edge_kind: String,
    pub confidence: f32,
    pub quote: String,
}

/// Phase 3 (E v2): input for cross-document semantic linking. Both lists are index-ordered;
/// returned links refer to concepts by their position in these vectors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticLinkInput {
    /// Concepts from the document just ingested.
    pub new_concepts: Vec<String>,
    /// Concepts already in the graph, from other documents.
    pub existing_concepts: Vec<String>,
}

/// One cross-document relationship the LLM judged meaningful.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticLink {
    /// Index into `SemanticLinkInput::new_concepts`.
    pub from_new: usize,
    /// Index into `SemanticLinkInput::existing_concepts`.
    pub to_existing: usize,
    /// EdgeKind as string, validated against the enum before mutation.
    pub edge_kind: String,
    pub confidence: f32,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticLinkOutput {
    pub links: Vec<SemanticLink>,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Low-level completion for callers that assemble their own prompt via
    /// [`crate::extract::prompts`]: send a system + user prompt, return the model's JSON text
    /// (markdown code fences stripped). Keeps prompt text in one tunable place rather than
    /// duplicated inside each provider.
    async fn complete(&self, system: &str, user: &str) -> Result<String, AxonMindError>;

    async fn extract_entities(
        &self,
        input: EntityExtractionInput,
    ) -> Result<EntityExtractionOutput, AxonMindError>;

    async fn extract_relations(
        &self,
        input: RelationExtractionInput,
    ) -> Result<RelationExtractionOutput, AxonMindError>;

    /// Phase 3 (E v2): find meaningful business relationships between concepts from the document
    /// just ingested and concepts already in the graph (from other documents). Returns only
    /// strong, specific links — this is what connects per-document clusters across documents.
    /// Returning an empty list is valid and expected when nothing relates.
    async fn link_concepts(
        &self,
        input: SemanticLinkInput,
    ) -> Result<SemanticLinkOutput, AxonMindError>;

    /// Phase 3: Generate a rationale string for `explain_kpi`.
    /// Output is cached by `(kpi_id, evidence_hash)` in the engine.
    async fn explain_kpi_rationale(
        &self,
        kpi_name: &str,
        evidence_quotes: &[String],
    ) -> Result<String, AxonMindError>;

    /// Transcribe an image to markdown text using the provider's vision capability.
    /// `bytes` is the raw image data; `mime_type` is e.g. "image/png".
    /// Default returns Err so providers that don't support vision fall back to Tesseract.
    async fn transcribe_image(
        &self,
        _bytes: &[u8],
        _mime_type: &str,
    ) -> Result<String, AxonMindError> {
        Err(AxonMindError::LlmProvider(
            "vision not supported by this provider".into(),
        ))
    }
}
