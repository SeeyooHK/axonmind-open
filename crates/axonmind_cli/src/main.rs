use axonmind_core::NodeId;
use axonmind_engine::{
    AxonMindEngine,
    config::{EngineConfig, WorkspaceManifest},
    ingest::{IngestOptions, IngestSource},
    query::{
        ExplainKpiInput, FocusKpiInput, GetEvidenceInput, GraphSearchInput, ImpactRadiusInput,
        SuggestActionsInput, TraceDecisionInput,
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
    }

    Ok(())
}
