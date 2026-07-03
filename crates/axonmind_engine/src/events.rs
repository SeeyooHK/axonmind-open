use axonmind_core::{EdgeId, EvidenceId, NodeId};
use serde::{Deserialize, Serialize};
/// Blocker B: EngineEvent enum — fully specified here.
///
/// All engine subsystems emit through `broadcast::Sender<EngineEvent>`.
/// Tauri adapter subscribes and forwards payloads to the frontend.
/// CLI may subscribe to print progress. Tests subscribe to assert side-effects.
///
/// No implementation may emit raw strings or create its own channel.
use std::path::PathBuf;

use crate::ingest::{IngestSummary, JobId};
use crate::store::CandidateId;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    NodeUpserted {
        node_id: NodeId,
    },
    NodeDeleted {
        node_id: NodeId,
    },
    EdgeUpserted {
        edge_id: EdgeId,
    },
    EdgeDeleted {
        edge_id: EdgeId,
    },
    EvidenceAdded {
        evidence_id: EvidenceId,
    },

    KpiCandidateProposed {
        candidate_id: CandidateId,
    },
    KpiCandidateResolved {
        candidate_id: CandidateId,
        status: CandidateStatus,
    },

    IngestBatchStarted {
        job_id: JobId,
        root_path: PathBuf,
        total_files: usize,
    },
    IngestStarted {
        job_id: JobId,
        path: PathBuf,
    },
    IngestProgress {
        job_id: JobId,
        processed: usize,
        total: Option<usize>,
    },
    IngestCompleted {
        job_id: JobId,
        summary: IngestSummary,
    },
    IngestFailed {
        job_id: JobId,
        error: String,
    },
    IngestFileStarted {
        job_id: JobId,
        path: PathBuf,
        index: usize,
        total: usize,
    },
    IngestFilePhase {
        job_id: JobId,
        path: PathBuf,
        phase: crate::store::IngestPhase,
    },
    IngestFileCompleted {
        job_id: JobId,
        path: PathBuf,
        status: String,
    },
    IngestFileFailed {
        job_id: JobId,
        path: PathBuf,
        error: String,
    },
    IngestStalled {
        job_id: JobId,
        path: PathBuf,
        elapsed_secs: usize,
    },
    IngestCancelled {
        job_id: JobId,
        processed: usize,
        total: usize,
    },

    /// Graph cache was rebuilt from SQLite (e.g. after dirty-flag recovery).
    CacheRebuilt,
}

/// Re-exported so subscribers don't need to import from store.
pub use crate::store::CandidateStatus;
