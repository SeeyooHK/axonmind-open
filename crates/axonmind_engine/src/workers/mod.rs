pub mod kpi_discovery;
pub mod kpi_recompute;

use crate::AxonMindEngine;

/// Spawn background workers based on `engine.config.workers`.
/// Safe to call multiple times — each call spawns another set of tasks,
/// so callers should call this exactly once (done automatically by `AxonMindEngine::open`).
pub(crate) fn start_workers(engine: &AxonMindEngine) {
    if engine.config.workers.enable_discovery_worker {
        kpi_discovery::spawn(
            engine.store.clone(),
            engine.graph_cache.clone(),
            engine.event_tx.clone(),
            engine.config.clone(),
        );
    }
    if engine.config.workers.enable_recompute_worker {
        kpi_recompute::spawn(
            engine.store.clone(),
            engine.graph_cache.clone(),
            engine.event_tx.clone(),
            engine.config.clone(),
        );
    }
}
