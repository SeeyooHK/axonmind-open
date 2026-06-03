mod mcp_server;

use axonmind_core::NodeId;
use axonmind_engine::{
    AxonMindEngine,
    config::{EngineConfig, WorkspaceManifest},
    ingest::{IngestOptions, IngestSource},
    query::{
        ExplainKpiInput, FocusKpiInput, GetEvidenceInput, GraphExportV1, GraphSearchInput,
        ImpactRadiusInput, ReasoningSearchInput, SuggestActionsInput, TraceDecisionInput,
    },
};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "axonmind", version, about = "axonmind-open CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new workspace at the given directory.
    Init {
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long, default_value = "default")]
        name: String,
    },
    /// Ingest files into the workspace.
    Index {
        path: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long, default_value_t = true)]
        recursive: bool,
        #[arg(long, default_value_t = false)]
        skip_unchanged: bool,
    },
    /// Run a graph query.
    Query {
        #[command(subcommand)]
        subcommand: QueryCommands,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Full-text search the graph.
    Search {
        query: String,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Export the full graph as JSON to stdout (or --out <file>).
    ExportJson {
        #[arg(long)]
        workspace: PathBuf,
        /// Write output to this file instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Import a graph export JSON file into the workspace.
    ImportJson {
        #[arg(long)]
        workspace: PathBuf,
        /// Path to the graph.json file produced by export-json.
        file: PathBuf,
    },
    /// Rebuild the FTS5 search_index from scratch.
    RebuildSearchIndex {
        #[arg(long)]
        workspace: PathBuf,
    },
    /// Rebuild the page index tables from stored blobs (no graph changes).
    RebuildPageIndex {
        #[arg(long)]
        workspace: PathBuf,
    },
    /// List indexed documents with their node ids (use with regenerate/remove).
    Documents {
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Re-extract one document from its stored blob, replacing its derived nodes/edges
    /// atomically. Use after an extractor or LLM change; does NOT pick up edits to the
    /// file on disk (it reads the retained blob). To diff a file edit, use remove + index.
    Regenerate {
        #[arg(long)]
        workspace: PathBuf,
        /// Document node id, e.g. "doc.2b1149b0" (see `documents`).
        #[arg(long)]
        node_id: String,
    },
    /// Remove a document and the concepts derived solely from it.
    Remove {
        #[arg(long)]
        workspace: PathBuf,
        /// Document node id, e.g. "doc.2b1149b0" (see `documents`).
        #[arg(long)]
        node_id: String,
        /// Also delete the retained blob if no other document references it.
        #[arg(long, default_value_t = false)]
        delete_blob: bool,
    },
    /// Show graph health statistics (node/edge/evidence counts, confidence, taint).
    Stats {
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Diff two graph export JSON files and report what changed.
    Diff {
        #[arg(long)]
        workspace: PathBuf,
        /// Path to the before export JSON (produced by export-json).
        before: PathBuf,
        /// Path to the after export JSON, or omit to diff before against the live graph.
        after: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Serve the workspace as an MCP server over stdio (newline-delimited JSON-RPC 2.0).
    Mcp {
        #[arg(long)]
        workspace: PathBuf,
    },
}

#[derive(Subcommand)]
enum QueryCommands {
    FocusKpi {
        kpi_id: String,
    },
    ExplainKpi {
        kpi_id: String,
        #[arg(long)]
        depth: Option<u32>,
    },
    GetEvidence {
        #[arg(long)]
        node_id: Option<String>,
        #[arg(long)]
        edge_id: Option<String>,
    },
    ImpactRadius {
        node_id: String,
        #[arg(long)]
        max_depth: Option<u32>,
    },
    TraceDecision {
        decision_node_id: String,
    },
    SuggestActions {
        kpi_id: String,
    },
    /// Vectorless BM25+LLM section search over indexed documents.
    ReasoningSearch {
        query: String,
        /// Restrict to specific document node ids (repeat for multiple: --doc id1 --doc id2).
        #[arg(long)]
        doc: Vec<String>,
        /// Max results to return (default: 20).
        #[arg(long)]
        limit: Option<usize>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { workspace, name } => {
            std::fs::create_dir_all(&workspace)?;
            let manifest_path = workspace.join("workspace.json");
            if manifest_path.exists() {
                anyhow::bail!("workspace already exists at {}", workspace.display());
            }
            let id = uuid::Uuid::new_v4().to_string();
            let manifest = WorkspaceManifest::new(id, name.clone());
            let json = serde_json::to_string_pretty(&manifest)?;
            std::fs::write(&manifest_path, json)?;
            // Open engine to run migrations and create DB
            let config = EngineConfig::from_workspace_dir(workspace.clone());
            AxonMindEngine::open(config).await?;
            println!(
                "Workspace '{}' initialized at {}",
                name,
                workspace.display()
            );
        }

        Commands::Index {
            path,
            workspace,
            recursive,
            skip_unchanged,
        } => {
            let config = EngineConfig::from_workspace_dir(workspace);
            let engine = AxonMindEngine::open(config).await?;
            let source = if path.is_dir() {
                IngestSource::Directory(path)
            } else {
                IngestSource::File(path)
            };
            let opts = IngestOptions {
                recursive,
                skip_unchanged,
                max_file_size_bytes: 50 * 1024 * 1024,
            };
            let summary = engine.ingest_sync(source, opts).await?;
            eprintln!(
                "Indexed: {} files, {} nodes, {} edges, {} evidence, {} skipped, {} errors",
                summary.files_processed,
                summary.nodes_created,
                summary.edges_created,
                summary.evidence_created,
                summary.files_skipped,
                summary.errors.len(),
            );
            for err in &summary.errors {
                eprintln!("  error: {err}");
            }
        }

        Commands::Query {
            subcommand,
            workspace,
            json,
        } => {
            let config = EngineConfig::from_workspace_dir(workspace);
            let engine = AxonMindEngine::open(config).await?;

            match subcommand {
                QueryCommands::FocusKpi { kpi_id } => {
                    let out = engine
                        .focus_kpi(FocusKpiInput {
                            kpi_id: NodeId(kpi_id),
                        })
                        .await?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        println!("KPI: {} ({})", out.kpi.name, out.kpi.id);
                        println!("  drivers:  {}", out.drivers.len());
                        println!("  blockers: {}", out.blockers.len());
                        println!("  risks:    {}", out.risks.len());
                        println!("  evidence: {}", out.evidence_count);
                    }
                }
                QueryCommands::ExplainKpi { kpi_id, depth } => {
                    let out = engine
                        .explain_kpi(ExplainKpiInput {
                            kpi_id: NodeId(kpi_id),
                            depth,
                        })
                        .await?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        println!("{}", out.rationale);
                        println!("\nconfidence: {:.0}%", out.confidence * 100.0);
                    }
                }
                QueryCommands::GetEvidence { node_id, edge_id } => {
                    let out = engine
                        .get_evidence(GetEvidenceInput {
                            node_id: node_id.map(|s| NodeId(s)),
                            edge_id: edge_id.map(|s| axonmind_core::EdgeId(s)),
                        })
                        .await?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        println!("{} evidence item(s):", out.evidence.len());
                        for ev in &out.evidence {
                            if let Some(q) = &ev.quote {
                                println!("  [{:.0}%] {q}", ev.confidence.0 * 100.0);
                            }
                        }
                    }
                }
                QueryCommands::ImpactRadius { node_id, max_depth } => {
                    let out = engine
                        .impact_radius(ImpactRadiusInput {
                            node_id: NodeId(node_id),
                            max_depth,
                        })
                        .await?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        println!("{} affected node(s):", out.affected.len());
                        for a in &out.affected {
                            println!("  depth {}: {} ({})", a.depth, a.node.name, a.node.id);
                        }
                    }
                }
                QueryCommands::TraceDecision { decision_node_id } => {
                    let out = engine
                        .trace_decision(TraceDecisionInput {
                            decision_node_id: NodeId(decision_node_id),
                        })
                        .await?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        println!("Decision: {} ({})", out.decision.name, out.decision.id);
                        println!("  caused by {} edge(s)", out.caused_by.len());
                        println!("  next actions: {}", out.next_actions.len());
                    }
                }
                QueryCommands::SuggestActions { kpi_id } => {
                    let out = engine
                        .suggest_actions(SuggestActionsInput {
                            kpi_id: NodeId(kpi_id),
                            status_filter: None,
                            include_unreviewed: None,
                        })
                        .await?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        println!("{} suggested action(s):", out.actions.len());
                        for a in &out.actions {
                            println!("  - {} ({})", a.name, a.id);
                        }
                    }
                }
                QueryCommands::ReasoningSearch { query, doc, limit } => {
                    let out = engine
                        .reasoning_search(ReasoningSearchInput {
                            query,
                            doc_node_ids: if doc.is_empty() { None } else { Some(doc) },
                            max_results: limit,
                        })
                        .await?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        println!(
                            "{} section(s) [reasoning_applied={}]:",
                            out.sections.len(),
                            out.reasoning_applied
                        );
                        for s in &out.sections {
                            println!("  [{}] {} — {}", s.doc_node_id, s.title, s.path.join(" › "));
                        }
                    }
                }
            }
        }

        Commands::Search {
            query,
            workspace,
            json,
        } => {
            let config = EngineConfig::from_workspace_dir(workspace);
            let engine = AxonMindEngine::open(config).await?;
            let out = engine
                .graph_search(GraphSearchInput {
                    query,
                    kinds: None,
                    limit: None,
                })
                .await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("{} result(s):", out.nodes.len());
                for node in &out.nodes {
                    println!("  [{:?}] {} ({})", node.kind, node.name, node.id);
                }
            }
        }

        Commands::ExportJson { workspace, out } => {
            let config = EngineConfig::from_workspace_dir(workspace);
            let engine = AxonMindEngine::open(config).await?;
            let export = engine.export_json().await?;
            let json = serde_json::to_string_pretty(&export)?;
            match out {
                Some(path) => std::fs::write(&path, &json)?,
                None => println!("{json}"),
            }
        }

        Commands::ImportJson { workspace, file } => {
            let config = EngineConfig::from_workspace_dir(workspace);
            let engine = AxonMindEngine::open(config).await?;
            let json = std::fs::read_to_string(&file)?;
            let export: axonmind_engine::query::GraphExportV1 = serde_json::from_str(&json)?;
            let summary = engine.import_export(export).await?;
            eprintln!(
                "Imported: {} nodes, {} edges, {} evidence, {} errors",
                summary.nodes_created,
                summary.edges_created,
                summary.evidence_created,
                summary.errors.len(),
            );
            for err in &summary.errors {
                eprintln!("  error: {err}");
            }
        }

        Commands::RebuildSearchIndex { workspace } => {
            let config = EngineConfig::from_workspace_dir(workspace);
            let engine = AxonMindEngine::open(config).await?;
            engine.rebuild_search_index().await?;
            println!("search index rebuilt");
        }

        Commands::RebuildPageIndex { workspace } => {
            let config = EngineConfig::from_workspace_dir(workspace);
            let engine = AxonMindEngine::open(config).await?;
            let (processed, skipped, errors) = engine.rebuild_page_index().await?;
            eprintln!(
                "page index rebuilt: {} processed, {} skipped, {} errors",
                processed,
                skipped,
                errors.len()
            );
            for err in &errors {
                eprintln!("  error: {err}");
            }
        }

        Commands::Documents { workspace, json } => {
            let config = EngineConfig::from_workspace_dir(workspace);
            let engine = AxonMindEngine::open(config).await?;
            let docs = engine.list_documents().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&docs)?);
            } else {
                println!("{} document(s):", docs.len());
                for d in &docs {
                    println!(
                        "  {} — {} ({} concepts, {} evidence)",
                        d.node_id, d.name, d.concept_count, d.evidence_count
                    );
                }
            }
        }

        Commands::Regenerate { workspace, node_id } => {
            let config = EngineConfig::from_workspace_dir(workspace);
            let engine = AxonMindEngine::open(config).await?;
            let summary = engine.regenerate_document(NodeId(node_id)).await?;
            eprintln!(
                "Regenerated: {} nodes, {} edges, {} evidence, {} errors",
                summary.nodes_created,
                summary.edges_created,
                summary.evidence_created,
                summary.errors.len(),
            );
            for err in &summary.errors {
                eprintln!("  error: {err}");
            }
        }

        Commands::Remove {
            workspace,
            node_id,
            delete_blob,
        } => {
            let config = EngineConfig::from_workspace_dir(workspace);
            let engine = AxonMindEngine::open(config).await?;
            engine.remove_document(NodeId(node_id), delete_blob).await?;
            eprintln!("Removed");
        }

        Commands::Stats { workspace, json } => {
            let config = EngineConfig::from_workspace_dir(workspace);
            let engine = AxonMindEngine::open(config).await?;
            let stats = engine.graph_stats().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&stats)?);
            } else {
                println!(
                    "nodes: {} ({} concept, {} document)",
                    stats.total_nodes, stats.concept_nodes, stats.document_nodes
                );
                println!("edges: {}", stats.total_edges);
                println!("evidence: {}", stats.total_evidence);
                println!("avg confidence: {:.1}%", stats.avg_confidence * 100.0);
                println!(
                    "tainted: {} nodes, {} edges",
                    stats.tainted_nodes, stats.tainted_edges
                );
                println!("review required: {} nodes", stats.review_required_nodes);
                if !stats.nodes_by_kind.is_empty() {
                    println!("by kind:");
                    for kc in &stats.nodes_by_kind {
                        println!("  {:?}: {}", kc.kind, kc.count);
                    }
                }
            }
        }

        Commands::Diff {
            workspace,
            before,
            after,
            json,
        } => {
            let before_json = std::fs::read_to_string(&before)?;
            let before_export: GraphExportV1 = serde_json::from_str(&before_json)?;

            let after_export: GraphExportV1 = if let Some(after_path) = after {
                let text = std::fs::read_to_string(&after_path)?;
                serde_json::from_str(&text)?
            } else {
                let config = EngineConfig::from_workspace_dir(workspace);
                let engine = AxonMindEngine::open(config).await?;
                engine.export_json().await?
            };

            let diff = axonmind_engine::query::diff_exports(&before_export, &after_export);

            if json {
                println!("{}", serde_json::to_string_pretty(&diff)?);
            } else {
                let s = &diff.summary;
                println!(
                    "nodes: +{} ~{} -{} | edges: +{} ~{} -{}",
                    s.nodes_added,
                    s.nodes_modified,
                    s.nodes_removed,
                    s.edges_added,
                    s.edges_modified,
                    s.edges_removed,
                );
                println!(
                    "since {} → {}",
                    diff.before_exported_at.format("%Y-%m-%dT%H:%M:%SZ"),
                    diff.after_exported_at.format("%Y-%m-%dT%H:%M:%SZ"),
                );
                for w in &diff.warnings {
                    eprintln!("  warning: {w}");
                }
            }
        }

        Commands::Mcp { workspace } => {
            let config = EngineConfig::from_workspace_dir(workspace);
            let engine = AxonMindEngine::open(config).await?;
            mcp_server::serve(engine).await?;
        }
    }

    Ok(())
}
