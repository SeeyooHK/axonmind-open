use super::IngestJob;
/// Phase 1: Bounded async ingest queue.
///
/// Implementation: `tokio::sync::mpsc` channel with capacity 1024.
/// The engine spawns a task that drains the queue, calls the parser,
/// runs extraction, and submits GraphMutations.
/// Progress is reported via `EngineEvent::IngestProgress`.
use axonmind_core::AxonMindError;

pub struct IngestQueue {
    tx: tokio::sync::mpsc::Sender<IngestJob>,
}

impl IngestQueue {
    pub fn new() -> (Self, tokio::sync::mpsc::Receiver<IngestJob>) {
        let (tx, rx) = tokio::sync::mpsc::channel(1024);
        (Self { tx }, rx)
    }

    pub async fn submit(&self, job: IngestJob) -> Result<(), AxonMindError> {
        self.tx.send(job).await.map_err(|_| AxonMindError::Ingest {
            message: "ingest queue closed".into(),
        })
    }
}
