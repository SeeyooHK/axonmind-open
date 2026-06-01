import React, { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useAxonMind, toGraphElements } from "@axonmind/react";
import type { AxonGraphElements, AxonGraphNode, AxonGraphEdge } from "@axonmind/react";
import { DropZone } from "./components/DropZone";
import { GenerationStaging, type StagedItem, type ItemStatus } from "./components/GenerationStaging";
import { FolderSummaryModal } from "./components/FolderSummaryModal";
import { FileVisualizationModal } from "./components/FileVisualizationModal";
import { SettingsModal } from "./components/SettingsModal";
import { DocumentsView } from "./components/DocumentsView";
import { ViewSwitch, type MapView } from "./components/ViewSwitch";
import { InspectorPanel } from "./components/InspectorPanel";
import { AxonGraphView } from "./components/graph/AxonGraphView";
import { BrainMapView } from "./components/graph/BrainMapView";
import axonmindLogo from "./assets/axonmind.svg";

type Phase = "upload" | "generating" | "map" | "documents";

interface DirFileEntry {
  path: string;
  supported: boolean;
  reject_reason: string | null;
}

interface FolderSummaryState {
  rootPaths: string[];
  accepted: Array<{ path: string; displayPath: string }>;
  rejected: Array<{ displayPath: string; reason: string }>;
}

function computeDisplayPath(filePath: string, dropRoot: string): string {
  const clean = (p: string) => p.replace(/\\/g, "/");
  const f = clean(filePath);
  const r = clean(dropRoot).replace(/\/$/, "");
  if (f === r) return f.split("/").pop() ?? f;
  const folderName = r.split("/").pop() ?? "";
  return folderName + "/" + f.slice(r.length + 1);
}

function defaultGenerationName(rootPaths: string[]): string {
  if (rootPaths.length === 0) return `Map ${new Date().toLocaleString()}`;
  const clean = (p: string) => p.replace(/\\/g, "/").replace(/\/$/, "");
  if (rootPaths.length === 1) {
    const p = clean(rootPaths[0]);
    return p.split("/").pop()!.replace(/\.[^.]+$/, "") || `Map ${new Date().toLocaleString()}`;
  }
  const parents = rootPaths.map(p => clean(p).split("/").slice(0, -1).join("/"));
  if (parents.every(p => p === parents[0])) {
    return parents[0].split("/").pop() ?? `Map ${new Date().toLocaleString()}`;
  }
  return `Map ${new Date().toLocaleString()}`;
}

export function AppShell() {
  const { transport } = useAxonMind();

  const [phase, setPhase] = useState<Phase>("upload");
  const [mapMode, setMapMode] = useState<"brain" | "graph">("brain");
  const [items, setItems] = useState<StagedItem[]>([]);
  const itemsRef = useRef<StagedItem[]>([]);
  useEffect(() => { itemsRef.current = items; }, [items]);

  const [generationName, setGenerationName] = useState("My Map");
  const [mapView, setMapView] = useState<MapView>("generation");
  const [allElements, setAllElements] = useState<AxonGraphElements>({ nodes: [], edges: [] });
  const [genElements, setGenElements] = useState<AxonGraphElements>({ nodes: [], edges: [] });
  const [selectedNode, setSelectedNode] = useState<AxonGraphNode | undefined>();
  const [selectedEdge, setSelectedEdge] = useState<AxonGraphEdge | undefined>();
  const [currentGenName, setCurrentGenName] = useState("This Generation");
  const [folderSummary, setFolderSummary] = useState<FolderSummaryState | null>(null);
  const [visualizingItem, setVisualizingItem] = useState<StagedItem | null>(null);
  const [dropError, setDropError] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [returnPhase, setReturnPhase] = useState<Phase>("upload");
  const [hasActiveKey, setHasActiveKey] = useState(false);

  // Navigate to the full-page Processed Files view, remembering where to return.
  const openDocuments = useCallback((from: Phase) => {
    setReturnPhase(from);
    setPhase("documents");
  }, []);

  // Refresh the "All Time" map after documents are removed/regenerated.
  const refreshAllElements = useCallback(() => {
    transport.exportJson()
      .then(exp => setAllElements(toGraphElements(exp)))
      .catch(() => {});
  }, [transport]);

  const refreshHasActiveKey = useCallback(() => {
    invoke<{ has_active_key: boolean }>("plugin:axonmind|has_active_api_key")
      .then(status => setHasActiveKey(status.has_active_key))
      .catch(() => setHasActiveKey(false));
  }, []);

  useEffect(() => {
    refreshHasActiveKey();
  }, [refreshHasActiveKey]);

  // ── Staging helpers ───────────────────────────────────────────────────────────

  const updateItemStatus = useCallback((id: string, patch: Partial<StagedItem>) => {
    setItems(prev => prev.map(i => i.id === id ? { ...i, ...patch } : i));
  }, []);

  const ingestItem = useCallback(async (item: StagedItem) => {
    updateItemStatus(item.id, { status: "ingesting" as ItemStatus });
    try {
      // Files from enumeration have extensions; folder fallback paths do not.
      const name = item.path.replace(/\\/g, "/").split("/").pop() ?? "";
      const recursive = !/\.[^.]+$/.test(name);
      const summary = await transport.indexPath(item.path, { recursive, skipUnchanged: false });
      console.log("[ingest]", item.displayPath, summary);
      updateItemStatus(item.id, { status: "ready", summary });
    } catch (err) {
      updateItemStatus(item.id, { status: "failed", error: String(err) });
    }
  }, [transport, updateItemStatus]);

  const addToStaging = useCallback((files: Array<{ path: string; displayPath: string }>, rootPaths: string[]) => {
    const existingPaths = new Set(itemsRef.current.map(i => i.path));
    const seen = new Set<string>();
    const newItems: StagedItem[] = files
      .filter(f => !existingPaths.has(f.path) && !seen.has(f.path) && (seen.add(f.path), true))
      .map(f => ({ id: `${f.path}-${Date.now()}`, path: f.path, displayPath: f.displayPath, status: "queued" as ItemStatus }));
    if (newItems.length === 0) return;
    setItems(prev => {
      if (prev.length === 0) setGenerationName(defaultGenerationName(rootPaths));
      const cur = new Set(prev.map(i => i.path));
      const actual = newItems.filter(i => !cur.has(i.path));
      return actual.length > 0 ? [...prev, ...actual] : prev;
    });
    for (const item of newItems) ingestItem(item);
  }, [ingestItem]);

  const onRemove = useCallback((id: string) => {
    setItems(prev => prev.filter(i => i.id !== id));
  }, []);

  // ── Drop handling ─────────────────────────────────────────────────────────────

  const onPaths = useCallback(async (rawPaths: string[]) => {
    setDropError(null);
    let results: Array<{ root: string; entries: DirFileEntry[]; isDir: boolean }>;
    try {
      results = await Promise.all(
        rawPaths.map(async root => {
          const entries = await invoke<DirFileEntry[]>("plugin:axonmind|list_dir_files", { path: root });
          const isDir = !(entries.length === 1 && entries[0].path === root);
          return { root, entries, isDir };
        })
      );
    } catch (err) {
      setDropError(`list_dir_files failed — restart bun tauri dev so Rust recompiles. ${String(err)}`);
      return;
    }

    const accepted: Array<{ path: string; displayPath: string }> = [];
    const rejected: Array<{ displayPath: string; reason: string }> = [];
    let hasDir = false;

    for (const { root, entries, isDir } of results) {
      if (isDir) hasDir = true;
      for (const entry of entries) {
        const displayPath = computeDisplayPath(entry.path, root);
        if (entry.supported) {
          accepted.push({ path: entry.path, displayPath });
        } else {
          rejected.push({ displayPath, reason: entry.reject_reason ?? "unsupported" });
        }
      }
    }

    if (hasDir) {
      setFolderSummary({ rootPaths: rawPaths, accepted, rejected });
    } else {
      addToStaging(accepted, rawPaths);
    }
  }, [addToStaging]);

  const confirmFolderSummary = useCallback(() => {
    if (!folderSummary) return;
    addToStaging(folderSummary.accepted, folderSummary.rootPaths);
    setFolderSummary(null);
  }, [folderSummary, addToStaging]);

  // ── Generate ──────────────────────────────────────────────────────────────────

  const onGenerate = useCallback(async () => {
    setPhase("generating");
    setCurrentGenName(generationName);
    const readyPaths = items.filter(i => i.status === "ready").map(i => i.path);
    try {
      const [genId, allExport] = await Promise.all([
        transport.createGenerationFromPaths(generationName, readyPaths),
        transport.exportJson(),
      ]);
      const genExport = await transport.exportGeneration(genId);
      setAllElements(toGraphElements(allExport));
      setGenElements(toGraphElements(genExport));
      setPhase("map");
    } catch {
      setPhase("upload");
    }
  }, [transport, generationName, items]);

  const onAddMore = useCallback(() => {
    setItems([]);
    setPhase("upload");
  }, []);

  // ── Selection ─────────────────────────────────────────────────────────────────

  const clearSelection = useCallback(() => {
    setSelectedNode(undefined);
    setSelectedEdge(undefined);
  }, []);

  const handleSelectNode = useCallback((node: AxonGraphNode) => {
    if (!node) { clearSelection(); return; }
    setSelectedNode(node);
    setSelectedEdge(undefined);
  }, [clearSelection]);

  const handleSelectEdge = useCallback((edge: AxonGraphEdge) => {
    if (!edge) { clearSelection(); return; }
    setSelectedEdge(edge);
    setSelectedNode(undefined);
  }, [clearSelection]);

  // ── Upload phase ──────────────────────────────────────────────────────────────

  if (phase === "upload") {
    return (
      <div style={{ minHeight: "100vh", display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", padding: "2.5rem 2rem" }}>
        <div style={{ width: "100%", maxWidth: 600 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 6 }}>
            <img src={axonmindLogo} style={{ width: 32, height: 32 }} alt="AxonMind Logo" />
            <h1 style={{ fontSize: 26, fontWeight: 700, margin: 0 }}>AxonMind</h1>
            <SettingsIcon hasActiveKey={hasActiveKey} onClick={() => setShowSettings(true)} />
            <ListIcon onClick={() => openDocuments("upload")} />
          </div>
          <p style={{ color: "#64748b", margin: "0 0 2rem", fontSize: 14 }}>
            Brain Map all documents dropped below!
          </p>
          <DropZone onPaths={onPaths} />
          {dropError && (
            <div style={{
              marginTop: 12, padding: "10px 14px", borderRadius: 8,
              background: "rgba(248,113,113,0.1)", border: "1px solid #f87171",
              color: "#fca5a5", fontSize: 12, fontFamily: "monospace", wordBreak: "break-word",
            }}>
              {dropError}
            </div>
          )}
          {items.length > 0 && (
            <div style={{ marginTop: "1.5rem" }}>
              <GenerationStaging
                items={items}
                generationName={generationName}
                onNameChange={setGenerationName}
                onGenerate={onGenerate}
                onRemove={onRemove}
                onVisualize={setVisualizingItem}
                hasActiveKey={hasActiveKey}
              />
            </div>
          )}
        </div>

        {folderSummary && (
          <FolderSummaryModal
            acceptedCount={folderSummary.accepted.length}
            rejected={folderSummary.rejected}
            onConfirm={confirmFolderSummary}
            onCancel={() => setFolderSummary(null)}
          />
        )}

        {visualizingItem && (
          <FileVisualizationModal item={visualizingItem} onClose={() => setVisualizingItem(null)} />
        )}

        {showSettings && (
          <SettingsModal
            onClose={() => setShowSettings(false)}
            onKeysChanged={refreshHasActiveKey}
          />
        )}

      </div>
    );
  }

  // ── Documents phase (full-page Processed Files view) ────────────────────────────

  if (phase === "documents") {
    return (
      <DocumentsView
        onBack={() => setPhase(returnPhase)}
        onChanged={refreshAllElements}
        elements={allElements}
      />
    );
  }

  // ── Generating phase ──────────────────────────────────────────────────────────

  if (phase === "generating") {
    return (
      <div style={{ height: "100vh", display: "flex", alignItems: "center", justifyContent: "center" }}>
        <div style={{ textAlign: "center" }}>
          <div style={{ fontSize: 52, marginBottom: 16 }}>🧠</div>
          <p style={{ color: "#64748b", margin: 0 }}>Building knowledge graph…</p>
        </div>
      </div>
    );
  }

  // ── Map phase ─────────────────────────────────────────────────────────────────

  const activeElements = mapView === "generation" ? genElements : allElements;

  return (
    <div style={{ height: "100vh", display: "flex", flexDirection: "column" }}>
      <div style={{
        flexShrink: 0, padding: "10px 18px",
        display: "flex", alignItems: "center", gap: 14,
        borderBottom: "1px solid #1e293b",
      }}>
        <span style={{ color: "#f1f5f9", fontWeight: 700, fontSize: 16 }}>AxonMind</span>
        <SettingsIcon hasActiveKey={hasActiveKey} onClick={() => setShowSettings(true)} />
        <ListIcon onClick={() => openDocuments("map")} />
        <ViewSwitch view={mapView} onChange={setMapView} generationName={currentGenName} />
        <div style={{ display: "flex", border: "1px solid #334155", borderRadius: 8, overflow: "hidden" }}>
          {(["brain", "graph"] as const).map(m => (
            <button
              key={m}
              onClick={() => setMapMode(m)}
              style={{ padding: "5px 12px", border: "none", background: mapMode === m ? "#1d4ed8" : "transparent", color: mapMode === m ? "#fff" : "#94a3b8", fontSize: 12, cursor: "pointer" }}
            >
              {m === "brain" ? "Brain Map" : "Graph"}
            </button>
          ))}
        </div>
        <span style={{ color: "#334155", fontSize: 12, marginLeft: 4 }}>
          {activeElements.nodes.length} nodes · {activeElements.edges.length} edges
        </span>
        <div style={{ flex: 1 }} />
        <button
          onClick={onAddMore}
          style={{
            padding: "5px 14px", borderRadius: 8,
            border: "1px solid #334155", background: "transparent",
            color: "#94a3b8", fontSize: 13, cursor: "pointer",
          }}
        >
          + Add Documents
        </button>
      </div>

      <div style={{ flex: 1, display: "flex", overflow: "hidden" }}>
        <div style={{ flex: 1, position: "relative" }}>
          {mapMode === "brain" ? (
            <BrainMapView
              elements={allElements}
              onSelectNode={handleSelectNode}
              onGoHome={() => setPhase("upload")}
              style={{ position: "absolute", inset: 0 }}
            />
          ) : activeElements.nodes.length === 0 ? (
            <div style={{ position: "absolute", inset: 0, display: "flex", alignItems: "center", justifyContent: "center" }}>
              <p style={{ color: "#334155", fontSize: 14 }}>No nodes in this view. Try switching to All Time.</p>
            </div>
          ) : (
            <AxonGraphView
              elements={activeElements}
              onSelectNode={handleSelectNode}
              onSelectEdge={handleSelectEdge}
              onGoHome={() => setPhase("upload")}
              style={{ position: "absolute", inset: 0 }}
            />
          )}
        </div>
        <InspectorPanel node={selectedNode} edge={selectedEdge} onClose={clearSelection} />
      </div>

      {showSettings && (
        <SettingsModal
          onClose={() => setShowSettings(false)}
          onKeysChanged={refreshHasActiveKey}
        />
      )}

    </div>
  );
}

function ListIcon({ onClick }: { onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      title="Processed files"
      style={{
        background: "none", border: "none", cursor: "pointer", padding: 4, borderRadius: 4,
        color: "#94a3b8", display: "flex", alignItems: "center", transition: "color 0.15s",
      }}
    >
      <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
        <line x1="8" y1="6" x2="21" y2="6" />
        <line x1="8" y1="12" x2="21" y2="12" />
        <line x1="8" y1="18" x2="21" y2="18" />
        <line x1="3" y1="6" x2="3.01" y2="6" />
        <line x1="3" y1="12" x2="3.01" y2="12" />
        <line x1="3" y1="18" x2="3.01" y2="18" />
      </svg>
    </button>
  );
}

function SettingsIcon({ hasActiveKey, onClick }: { hasActiveKey: boolean; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      title={hasActiveKey ? "API Settings (active)" : "API Settings — no active key"}
      style={{
        background: "none", border: "none", cursor: "pointer", padding: 4, borderRadius: 4,
        color: hasActiveKey ? "#2ee4bb" : "#ef4444",
        display: "flex", alignItems: "center",
        transition: "color 0.15s",
      }}
    >
      <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
        <circle cx="12" cy="12" r="3" />
        <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
      </svg>
    </button>
  );
}
