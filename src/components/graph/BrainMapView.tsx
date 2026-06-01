import React, { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AxonGraphElements, AxonGraphNode } from "@axonmind/react";

// Mirrors the Rust `SuggestedSummary` (serde).
export interface Category {
  label: string;
  headline_node_id: string;
  member_node_ids: string[];
}
export interface Summary {
  categories: Category[];
  source: string;
  /** node id → display name, supplied by the engine so labels are exact without the full graph. */
  labels?: Record<string, string>;
}

interface MeasureResolution {
  type: string;
  state: "resolved" | "unknown";
  value?: number | null;
  unit?: string | null;
  confidence?: number | null;
  observed_at?: string | null;
  explanation?: string | null;
  evidence_ids: string[];
  evidence_lineage: EvidenceLineageItem[];
  lineage_gaps: LineageGap[];
  supporting_nodes: SupportingNodeRef[];
}
interface SupportingNodeRef {
  node_id: string;
  label: string;
  kind: string;
  role: string;
}
interface EvidenceLineageItem {
  evidence_id: string;
  source_node_id: string;
  source_node_name: string;
  source_type: string;
  source_path?: string | null;
  row_ref?: string | null;
  quote?: string | null;
  timestamp?: string | null;
}
interface LineageGap {
  code: string;
  message: string;
}
interface HealthResolution {
  state: "good" | "watch" | "at_risk" | "unknown";
  explanation?: string | null;
}
interface EffectiveLensContext {
  effective_selector: unknown;
  effective_period: string;
  effective_as_of: string;
}
interface LensResolution {
  lens_id: string;
  label: string;
  child_lens_ids: string[];
  selected_node_ids: string[];
  effective_context: EffectiveLensContext;
  measure_rule: unknown;
  measure: MeasureResolution;
  health: HealthResolution;
}
interface SummaryResolution {
  summary_id: string;
  summary_name: string;
  source: string;
  lenses: LensResolution[];
}

interface SummaryConfigSnapshot {
  config_path: string;
  config_exists: boolean;
  config: {
    summary: {
      id: string;
      name: string;
      default_period?: string | null;
      default_as_of?: string | null;
      lenses: string[];
    };
    lenses: Array<{
      id: string;
      label: string;
      hidden?: boolean | null;
      headline_node_id?: string | null;
      member_node_ids?: string[];
      measure: Record<string, unknown>;
      health?: Record<string, unknown> | null;
    }>;
  };
}

interface LensEditorDraft {
  id: string;
  label: string;
  hidden: boolean;
  period: string;
  asOf: string;
  thresholdEnabled: boolean;
  greenLt: string;
  amberLt: string;
  amberGte: string;
  redGte: string;
}

interface Props {
  /** Elements used to resolve node ids to labels at the drill level. Optional — ids are
   *  prettified as a fallback when omitted (e.g. when rendered from the file-list modal). */
  elements?: AxonGraphElements;
  /** Pre-fetched summary. When provided, the view renders it and does not fetch on mount
   *  (and hides the Regenerate control, since the parent owns generation). */
  initialSummary?: Summary;
  /** Document ids to scope a self-issued (re)generate to; omit for the whole graph. */
  scopeDocIds?: string[];
  onSelectNode?: (node: AxonGraphNode) => void;
  onGoHome?: () => void;
  style?: React.CSSProperties;
}

const PALETTE = [
  "#2563eb", "#0891b2", "#7c3aed", "#db2777", "#ea580c",
  "#16a34a", "#ca8a04", "#dc2626", "#0d9488", "#9333ea",
];

interface Circle {
  id: string;          // category index (level 0) or node id (level 1)
  x: number;
  y: number;
  label: string;
  sub?: string;
  tooltip?: string;
  color: string;
  onClick: () => void;
}

export function BrainMapView({ elements, initialSummary, scopeDocIds, onSelectNode, onGoHome, style }: Props) {
  const [summary, setSummary] = useState<Summary | null>(initialSummary ?? null);
  const [summaryResolution, setSummaryResolution] = useState<SummaryResolution | null>(null);
  const [configSnapshot, setConfigSnapshot] = useState<SummaryConfigSnapshot | null>(null);
  const [editOpen, setEditOpen] = useState(false);
  const [editSaving, setEditSaving] = useState(false);
  const [summaryNameDraft, setSummaryNameDraft] = useState("");
  const [defaultPeriodDraft, setDefaultPeriodDraft] = useState("latest");
  const [defaultAsOfDraft, setDefaultAsOfDraft] = useState("latest");
  const [lensDrafts, setLensDrafts] = useState<LensEditorDraft[]>([]);
  const [lensPath, setLensPath] = useState<LensResolution[]>([]);
  const [childLensResolutions, setChildLensResolutions] = useState<LensResolution[]>([]);
  const [loading, setLoading] = useState(!initialSummary);
  const [error, setError] = useState<string | null>(null);
  const [activeCat, setActiveCat] = useState<number | null>(null); // null = top level

  const containerRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });

  const nodesById = useMemo(() => {
    const m = new Map<string, AxonGraphNode>();
    for (const n of elements?.nodes ?? []) m.set(n.id, n);
    return m;
  }, [elements]);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const update = () => setSize({ w: el.clientWidth, h: el.clientHeight });
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Self-fetch only when the parent didn't hand us a summary.
  useEffect(() => { if (!initialSummary) void generate(); }, []);
  // Keep in sync if the parent swaps the summary in (e.g. modal reopened with a new result).
  useEffect(() => {
    if (initialSummary) {
      setSummary(initialSummary);
      setActiveCat(null);
      setLensPath([]);
      setChildLensResolutions([]);
      if (scopeDocIds && scopeDocIds.length > 0) {
        void loadSummaryResolution(scopeDocIds).then((resolved) => {
          if (resolved) setSummary(summaryFromResolution(resolved, initialSummary.labels));
        });
      } else {
        void loadSummaryResolution().then((resolved) => {
          if (resolved) setSummary(summaryFromResolution(resolved, initialSummary.labels));
        });
      }
    }
  }, [initialSummary, scopeDocIds]);

  async function generate() {
    setLoading(true);
    setError(null);
    try {
      const docIds = scopeDocIds && scopeDocIds.length ? scopeDocIds : undefined;
      if (docIds && docIds.length > 0) {
        const suggested = await invoke<Summary>("plugin:axonmind|suggest_summary", { docIds });
        const resolved = await loadSummaryResolution(docIds);
        setSummary(resolved ? summaryFromResolution(resolved, suggested.labels) : suggested);
      } else {
        const [snap, resolved] = await Promise.all([
          invoke<SummaryConfigSnapshot>("plugin:axonmind|get_brain_map_default_config"),
          invoke<SummaryResolution>("plugin:axonmind|resolve_brain_map_default_summary"),
        ]);
        setConfigSnapshot(snap);
        setSummaryResolution(resolved);
        setSummary(summaryFromResolution(resolved));
      }
      setActiveCat(null);
      setLensPath([]);
      setChildLensResolutions([]);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  async function loadSummaryResolution(docIds?: string[]): Promise<SummaryResolution | null> {
    try {
      const res = await invoke<SummaryResolution>("plugin:axonmind|resolve_brain_map_default_summary", {
        docIds: docIds && docIds.length ? docIds : undefined,
      });
      setSummaryResolution(res);
      return res;
    } catch {
      // Keep the graph usable even if measure resolution fails.
      setSummaryResolution(null);
      return null;
    }
  }

  async function loadLensChildren(parentLensId: string) {
    try {
      const res = await invoke<LensResolution[]>("plugin:axonmind|resolve_brain_map_lens_children", {
        parentLensId,
      });
      setChildLensResolutions(res);
    } catch {
      setChildLensResolutions([]);
    }
  }

  async function loadConfigSnapshot() {
    const snap = await invoke<SummaryConfigSnapshot>("plugin:axonmind|get_brain_map_default_config");
    setConfigSnapshot(snap);

    const lensById = new Map(snap.config.lenses.map((l) => [l.id, l]));
    const ordered = snap.config.summary.lenses
      .map((id) => lensById.get(id))
      .filter((x): x is NonNullable<typeof x> => !!x);
    setSummaryNameDraft(snap.config.summary.name || "Default Summary");
    setDefaultPeriodDraft(snap.config.summary.default_period || "latest");
    setDefaultAsOfDraft(snap.config.summary.default_as_of || "latest");
    setLensDrafts(
      ordered.map((lens) => {
        const health = (lens.health ?? {}) as Record<string, unknown>;
        const thresholdEnabled = health.type === "threshold";
        return {
          id: lens.id,
          label: lens.label,
          hidden: !!lens.hidden,
          period: String((lens.measure?.period as string | undefined) ?? "latest"),
          asOf: String((lens.measure?.as_of as string | undefined) ?? "latest"),
          thresholdEnabled,
          greenLt: health.green_lt != null ? String(health.green_lt) : "",
          amberLt: health.amber_lt != null ? String(health.amber_lt) : "",
          amberGte: health.amber_gte != null ? String(health.amber_gte) : "",
          redGte: health.red_gte != null ? String(health.red_gte) : "",
        };
      })
    );
  }

  async function openEditor() {
    try {
      await loadConfigSnapshot();
      setEditOpen(true);
    } catch (e) {
      setError(String(e));
    }
  }

  function moveLens(idx: number, dir: -1 | 1) {
    setLensDrafts((prev) => {
      const next = [...prev];
      const swap = idx + dir;
      if (swap < 0 || swap >= next.length) return prev;
      const tmp = next[idx];
      next[idx] = next[swap];
      next[swap] = tmp;
      return next;
    });
  }

  async function saveEditor() {
    if (editSaving) return;
    setEditSaving(true);
    setError(null);
    try {
      await invoke<SummaryConfigSnapshot>("plugin:axonmind|update_brain_map_default_config", {
        edit: {
          summary_name: summaryNameDraft,
          default_period: defaultPeriodDraft,
          default_as_of: defaultAsOfDraft,
          lens_order: lensDrafts.map((l) => l.id),
          lenses: lensDrafts.map((l) => {
            const health = l.thresholdEnabled
              ? {
                type: "threshold",
                ...(l.greenLt.trim() !== "" ? { green_lt: Number(l.greenLt) } : {}),
                ...(l.amberLt.trim() !== "" ? { amber_lt: Number(l.amberLt) } : {}),
                ...(l.amberGte.trim() !== "" ? { amber_gte: Number(l.amberGte) } : {}),
                ...(l.redGte.trim() !== "" ? { red_gte: Number(l.redGte) } : {}),
              }
              : undefined;
            return {
              lens_id: l.id,
              label: l.label,
              hidden: l.hidden,
              measure_period: l.period,
              measure_as_of: l.asOf,
              ...(health ? { health } : {}),
            };
          }),
        },
      });
      await generate();
      await loadConfigSnapshot();
      setEditOpen(false);
    } catch (e) {
      setError(String(e));
    } finally {
      setEditSaving(false);
    }
  }

  async function restoreDefaults() {
    if (editSaving) return;
    setEditSaving(true);
    setError(null);
    try {
      await invoke<SummaryConfigSnapshot>("plugin:axonmind|restore_brain_map_default_config");
      await generate();
      await loadConfigSnapshot();
      setEditOpen(false);
    } catch (e) {
      setError(String(e));
    } finally {
      setEditSaving(false);
    }
  }

  const isScopedSummary = !!(scopeDocIds && scopeDocIds.length > 0)
    || summaryResolution?.summary_id === "scoped";

  async function openLens(lens: LensResolution) {
    setActiveCat(null);
    const next = [...lensPath, lens];
    setLensPath(next);
    if (!isScopedSummary && lens.child_lens_ids.length > 0) {
      await loadLensChildren(lens.lens_id);
    } else {
      setChildLensResolutions([]);
    }
  }

  async function goLensBack() {
    if (lensPath.length === 0) return;
    const next = lensPath.slice(0, -1);
    setLensPath(next);
    if (next.length === 0) {
      setChildLensResolutions([]);
      return;
    }
    const hub = next[next.length - 1];
    if (!isScopedSummary && hub.child_lens_ids.length > 0) {
      await loadLensChildren(hub.lens_id);
    } else {
      setChildLensResolutions([]);
    }
  }

  async function jumpToLensDepth(depth: number) {
    if (depth < 0) {
      setLensPath([]);
      setChildLensResolutions([]);
      setActiveCat(null);
      return;
    }

    const next = lensPath.slice(0, depth + 1);
    setLensPath(next);
    setActiveCat(null);
    if (next.length === 0) {
      setChildLensResolutions([]);
      return;
    }
    const hub = next[next.length - 1];
    if (!isScopedSummary && hub.child_lens_ids.length > 0) {
      await loadLensChildren(hub.lens_id);
    } else {
      setChildLensResolutions([]);
    }
  }

  // Prefer the engine-supplied exact label, then a node from `elements`, then a prettified id.
  const nameOf = (id: string) =>
    summary?.labels?.[id] ?? nodesById.get(id)?.label ?? id.replace(/^[a-z0-9]+\./i, "").replace(/_/g, " ");

  const cx = size.w / 2;
  const cy = size.h / 2;
  const R = Math.max(140, Math.min(size.w, size.h) * 0.36);

  function ringPos(i: number, n: number) {
    const angle = -Math.PI / 2 + (i * 2 * Math.PI) / Math.max(1, n);
    return { x: cx + R * Math.cos(angle), y: cy + R * Math.sin(angle) };
  }

  // Build hub + ring for the current level.
  let hubLabel = "Brain Map";
  let hubSub: string | undefined = summary ? summary.source : undefined;
  let hubOnClick: (() => void) | undefined;
  const circles: Circle[] = [];
  const currentLens = lensPath.length > 0 ? lensPath[lensPath.length - 1] : null;
  const focusedLensResolution = currentLens
    ?? (activeCat !== null ? summaryResolution?.lenses?.[activeCat] ?? null : null);

  if (summary && size.w > 0) {
    if (currentLens) {
      hubLabel = currentLens.label;
      hubOnClick = () => { void goLensBack(); };

      if (childLensResolutions.length > 0) {
        hubSub = `${childLensResolutions.length} child lens${childLensResolutions.length === 1 ? "" : "es"} · click to go back`;
        childLensResolutions.forEach((child, i) => {
          const p = ringPos(i, childLensResolutions.length);
          circles.push({
            id: child.lens_id,
            x: p.x,
            y: p.y,
            label: child.label,
            sub: topLevelSubLabel(child),
            tooltip: topLevelTooltip(child.label, child),
            color: colorForHealth(child.health.state),
            onClick: () => { void openLens(child); },
          });
        });
      } else {
        hubSub = `${currentLens.selected_node_ids.length} items · click to go back`;
        currentLens.selected_node_ids.forEach((id, i) => {
          const p = ringPos(i, currentLens.selected_node_ids.length);
          circles.push({
            id,
            x: p.x,
            y: p.y,
            label: nameOf(id),
            color: "#475569",
            onClick: () => { const n = nodesById.get(id); if (n) onSelectNode?.(n); },
          });
        });
      }
    } else if (activeCat === null) {
      const cats = summary.categories;
      cats.forEach((c, i) => {
        const p = ringPos(i, cats.length);
        const lensResolution = summaryResolution?.lenses?.[i];
        const sub = lensResolution
          ? topLevelSubLabel(lensResolution)
          : `${c.member_node_ids.length} item${c.member_node_ids.length === 1 ? "" : "s"}`;
        const color = lensResolution?.health
          ? colorForHealth(lensResolution.health.state)
          : PALETTE[i % PALETTE.length];
        circles.push({
          id: String(i),
          x: p.x, y: p.y,
          label: c.label,
          sub,
          tooltip: lensResolution ? topLevelTooltip(c.label, lensResolution) : undefined,
          color,
          onClick: () => {
            if (!isScopedSummary && lensResolution && lensResolution.child_lens_ids.length > 0) {
              void openLens(lensResolution);
            } else {
              setActiveCat(i);
            }
          },
        });
      });
    } else {
      const cat = summary.categories[activeCat];
      hubLabel = cat.label;
      hubSub = `${cat.member_node_ids.length} items · click to go back`;
      hubOnClick = () => setActiveCat(null);
      cat.member_node_ids.forEach((id, i) => {
        const p = ringPos(i, cat.member_node_ids.length);
        const isHeadline = id === cat.headline_node_id;
        circles.push({
          id,
          x: p.x, y: p.y,
          label: nameOf(id),
          color: isHeadline ? PALETTE[activeCat % PALETTE.length] : "#475569",
          onClick: () => { const n = nodesById.get(id); if (n) onSelectNode?.(n); },
        });
      });
    }
  }

  const hubSize = 132;
  const circleSize = activeCat === null && lensPath.length === 0 ? 108 : 92;
  const activeCatLabel = activeCat !== null && summary ? summary.categories[activeCat]?.label : null;

  return (
    <div ref={containerRef} style={{ position: "relative", width: "100%", height: "100%", background: "#0b1120", overflow: "hidden", ...style }}>
      {/* connector lines */}
      <svg style={{ position: "absolute", inset: 0, width: "100%", height: "100%", pointerEvents: "none" }}>
        {circles.map(c => (
          <line key={c.id} x1={cx} y1={cy} x2={c.x} y2={c.y} stroke="#1e293b" strokeWidth={2} />
        ))}
      </svg>

      {/* hub */}
      {summary && size.w > 0 && (
        <CircleNode
          x={cx} y={cy} size={hubSize}
          label={hubLabel} sub={hubSub}
          bg="#1e293b" border="#334155" bold
          onClick={hubOnClick}
        />
      )}

      {/* ring circles */}
      {circles.map(c => (
        <CircleNode
          key={c.id}
          x={c.x} y={c.y} size={circleSize}
          label={c.label} sub={c.sub}
          title={c.tooltip}
          bg={c.color} border={c.color}
          onClick={c.onClick}
        />
      ))}

      {/* controls */}
      <div style={{ position: "absolute", top: 14, left: 14, display: "flex", gap: 8, zIndex: 10 }}>
        {onGoHome && <button onClick={onGoHome} style={ctrlBtn} title="Home">⌂</button>}
        {!initialSummary && (
          <button onClick={() => void generate()} disabled={loading} style={ctrlBtn}>
            {loading ? "Generating…" : "Regenerate"}
          </button>
        )}
        {!initialSummary && !isScopedSummary && (
          <button onClick={() => void openEditor()} style={ctrlBtn} title="Edit Summary">
            Edit
          </button>
        )}
        {(activeCat !== null || lensPath.length > 0) && (
          <button
            onClick={() => {
              if (lensPath.length > 0) {
                void goLensBack();
              } else {
                setActiveCat(null);
              }
            }}
            style={ctrlBtn}
          >
            ‹ Back
          </button>
        )}
      </div>

      {/* breadcrumb trail */}
      {(lensPath.length > 0 || activeCatLabel) && (
        <div style={{
          position: "absolute",
          top: 52,
          left: 14,
          display: "flex",
          alignItems: "center",
          gap: 6,
          zIndex: 10,
          maxWidth: "65%",
          overflow: "hidden",
          whiteSpace: "nowrap",
          textOverflow: "ellipsis",
          padding: "4px 8px",
          borderRadius: 8,
          border: "1px solid #334155",
          background: "rgba(15,23,42,0.85)",
        }}>
          <button
            onClick={() => { void jumpToLensDepth(-1); }}
            style={crumbBtn}
            title="Back to Brain Map root"
          >
            Brain Map
          </button>
          {lensPath.map((lens, i) => (
            <React.Fragment key={lens.lens_id}>
              <span style={{ color: "#475569", fontSize: 11 }}>›</span>
              {i === lensPath.length - 1 ? (
                <span style={{ color: "#cbd5e1", fontSize: 11 }}>{lens.label}</span>
              ) : (
                <button
                  onClick={() => { void jumpToLensDepth(i); }}
                  style={crumbBtn}
                  title={`Jump to ${lens.label}`}
                >
                  {lens.label}
                </button>
              )}
            </React.Fragment>
          ))}
          {lensPath.length === 0 && activeCatLabel && (
            <>
              <span style={{ color: "#475569", fontSize: 11 }}>›</span>
              <span style={{ color: "#cbd5e1", fontSize: 11 }}>{activeCatLabel}</span>
            </>
          )}
        </div>
      )}

      {/* top-level explanation panel */}
      {activeCat === null && lensPath.length === 0 && summary && summaryResolution && summaryResolution.lenses.length > 0 && (
        <div style={{
          position: "absolute",
          top: 14,
          right: 14,
          width: 300,
          maxWidth: "42vw",
          maxHeight: "58vh",
          overflow: "auto",
          border: "1px solid #334155",
          borderRadius: 10,
          background: "rgba(15,23,42,0.92)",
          padding: 10,
          zIndex: 10,
          boxSizing: "border-box",
        }}>
          <div style={{ color: "#cbd5e1", fontSize: 12, fontWeight: 700, marginBottom: 8 }}>
            Lens Status
          </div>
          {summaryResolution.lenses.map((lens, i) => (
            <div key={lens.lens_id} style={{
              borderTop: i === 0 ? "none" : "1px solid #1e293b",
              paddingTop: i === 0 ? 0 : 8,
              marginTop: i === 0 ? 0 : 8,
            }}>
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <span style={{
                  width: 8,
                  height: 8,
                  borderRadius: 999,
                  background: colorForHealth(lens.health.state),
                  flexShrink: 0,
                }} />
                <span style={{ color: "#e2e8f0", fontSize: 12, fontWeight: 600 }}>
                  {summary.categories[i]?.label ?? lens.label}
                </span>
              </div>
              <div style={{ color: "#94a3b8", fontSize: 11, marginTop: 4 }}>
                {topLevelSubLabel(lens)}
              </div>
              {lens.measure.explanation && (
                <div style={{ color: "#64748b", fontSize: 10, marginTop: 4, lineHeight: 1.35 }}>
                  {lens.measure.explanation}
                </div>
              )}
              {lens.health.explanation && (
                <div style={{ color: "#64748b", fontSize: 10, marginTop: 3, lineHeight: 1.35 }}>
                  {lens.health.explanation}
                </div>
              )}
            </div>
          ))}
        </div>
      )}

      {/* focused-lens explainability panel */}
      {focusedLensResolution && (
        <div style={{
          position: "absolute",
          top: 14,
          right: 14,
          width: 340,
          maxWidth: "46vw",
          maxHeight: "70vh",
          overflow: "auto",
          border: "1px solid #334155",
          borderRadius: 10,
          background: "rgba(15,23,42,0.92)",
          padding: 10,
          zIndex: 10,
          boxSizing: "border-box",
        }}>
          <div style={{ color: "#cbd5e1", fontSize: 12, fontWeight: 700, marginBottom: 8 }}>
            Why This Lens
          </div>
          <div style={{ color: "#e2e8f0", fontSize: 12, fontWeight: 600, marginBottom: 6 }}>
            {focusedLensResolution.label}
          </div>
          <div style={{ color: "#94a3b8", fontSize: 11, marginBottom: 6 }}>
            {topLevelSubLabel(focusedLensResolution)}
          </div>
          <div style={{ color: "#64748b", fontSize: 10, marginBottom: 8 }}>
            period: {focusedLensResolution.effective_context?.effective_period ?? "latest"} · as_of: {focusedLensResolution.effective_context?.effective_as_of ?? "latest"}
          </div>

          <PanelBlock title="Effective Selector" body={prettyJson(focusedLensResolution.effective_context?.effective_selector)} />
          <PanelBlock title="Measure Rule" body={prettyJson(focusedLensResolution.measure_rule)} />
          <PanelBlock
            title="Measure Metadata"
            body={`confidence: ${focusedLensResolution.measure.confidence ?? "n/a"}\nobserved_at: ${focusedLensResolution.measure.observed_at ?? "n/a"}`}
          />
          <PanelBlock
            title="Evidence IDs"
            body={focusedLensResolution.measure.evidence_ids.length > 0
              ? focusedLensResolution.measure.evidence_ids.join("\n")
              : "none"}
          />
          <PanelBlock
            title="Evidence Trail"
            body={renderEvidenceLineage(focusedLensResolution.measure.evidence_lineage)}
          />
          <PanelBlock
            title="Lineage Gaps"
            body={renderLineageGaps(focusedLensResolution.measure.lineage_gaps)}
          />
          <div style={{ marginTop: 8 }}>
            <div style={{ color: "#94a3b8", fontSize: 10, marginBottom: 4 }}>Supporting Nodes</div>
            <div style={{
              background: "#020617",
              border: "1px solid #1e293b",
              borderRadius: 6,
              padding: 8,
              display: "flex",
              flexDirection: "column",
              gap: 6,
            }}>
              {focusedLensResolution.measure.supporting_nodes.length === 0 && (
                <div style={{ color: "#64748b", fontSize: 10 }}>none</div>
              )}
              {focusedLensResolution.measure.supporting_nodes.map((n, i) => {
                const graphNode = nodesById.get(n.node_id);
                return (
                  <div key={`${n.node_id}:${i}`} style={{ display: "flex", alignItems: "center", gap: 6 }}>
                    <button
                      disabled={!graphNode}
                      onClick={() => { if (graphNode) onSelectNode?.(graphNode); }}
                      style={{
                        ...miniBtn,
                        opacity: graphNode ? 1 : 0.45,
                        cursor: graphNode ? "pointer" : "not-allowed",
                      }}
                      title={graphNode ? `Open ${n.label}` : "Node not available in current graph context"}
                    >
                      {n.label}
                    </button>
                    <span style={{ color: "#64748b", fontSize: 10 }}>{n.kind} · {n.role}</span>
                  </div>
                );
              })}
            </div>
          </div>
          <div style={{ marginTop: 8 }}>
            <div style={{ color: "#94a3b8", fontSize: 10, marginBottom: 4 }}>Evidence Trail (Interactive)</div>
            <div style={{
              background: "#020617",
              border: "1px solid #1e293b",
              borderRadius: 6,
              padding: 8,
              display: "flex",
              flexDirection: "column",
              gap: 8,
            }}>
              {focusedLensResolution.measure.evidence_lineage.length === 0 && (
                <div style={{ color: "#64748b", fontSize: 10 }}>none</div>
              )}
              {focusedLensResolution.measure.evidence_lineage.map((item) => {
                const sourceNode = nodesById.get(item.source_node_id);
                return (
                  <div key={item.evidence_id} style={{ borderTop: "1px solid #1e293b", paddingTop: 6 }}>
                    <div style={{ color: "#e2e8f0", fontSize: 11, fontWeight: 600 }}>
                      {item.source_node_name} ({item.source_type})
                    </div>
                    <div style={{ color: "#64748b", fontSize: 10, marginTop: 2 }}>id: {item.evidence_id}</div>
                    {item.row_ref && <div style={{ color: "#64748b", fontSize: 10 }}>row_ref: {item.row_ref}</div>}
                    {item.source_path && <div style={{ color: "#64748b", fontSize: 10, wordBreak: "break-word" }}>source_path: {item.source_path}</div>}
                    {item.quote && <div style={{ color: "#94a3b8", fontSize: 10, marginTop: 4, whiteSpace: "pre-wrap" }}>{item.quote}</div>}
                    <div style={{ display: "flex", gap: 6, marginTop: 6 }}>
                      <button
                        disabled={!sourceNode}
                        onClick={() => { if (sourceNode) onSelectNode?.(sourceNode); }}
                        style={{
                          ...miniBtn,
                          opacity: sourceNode ? 1 : 0.45,
                          cursor: sourceNode ? "pointer" : "not-allowed",
                        }}
                        title={sourceNode ? "Open source node in inspector" : "Source node not available in current graph context"}
                      >
                        Open Source Node
                      </button>
                      {item.quote && (
                        <button
                          onClick={() => void navigator.clipboard.writeText(item.quote ?? "")}
                          style={miniBtn}
                          title="Copy evidence quote"
                        >
                          Copy Quote
                        </button>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        </div>
      )}

      {/* summary config editor */}
      {editOpen && !isScopedSummary && (
        <div style={{
          position: "absolute",
          top: 14,
          right: 14,
          width: 420,
          maxWidth: "56vw",
          maxHeight: "80vh",
          overflow: "auto",
          border: "1px solid #334155",
          borderRadius: 10,
          background: "rgba(15,23,42,0.95)",
          padding: 10,
          zIndex: 12,
          boxSizing: "border-box",
          display: "flex",
          flexDirection: "column",
          gap: 10,
        }}>
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
            <div style={{ color: "#cbd5e1", fontSize: 12, fontWeight: 700 }}>Edit Summary</div>
            <button onClick={() => setEditOpen(false)} style={miniBtn}>Close</button>
          </div>

          <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8 }}>
            <label style={editorLabel}>
              <span>Summary Name</span>
              <input
                value={summaryNameDraft}
                onChange={(e) => setSummaryNameDraft(e.target.value)}
                style={editorInput}
              />
            </label>
            <label style={editorLabel}>
              <span>Default Period</span>
              <input
                value={defaultPeriodDraft}
                onChange={(e) => setDefaultPeriodDraft(e.target.value)}
                style={editorInput}
              />
            </label>
            <label style={editorLabel}>
              <span>Default As Of</span>
              <input
                value={defaultAsOfDraft}
                onChange={(e) => setDefaultAsOfDraft(e.target.value)}
                style={editorInput}
              />
            </label>
          </div>

          <div style={{ color: "#94a3b8", fontSize: 10, marginTop: 4 }}>Top Lenses</div>
          <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
            {lensDrafts.map((lens, idx) => (
              <div key={lens.id} style={{
                border: "1px solid #1e293b",
                borderRadius: 8,
                padding: 8,
                display: "flex",
                flexDirection: "column",
                gap: 8,
                background: "#020617",
              }}>
                <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                  <button style={miniBtn} onClick={() => moveLens(idx, -1)} disabled={idx === 0}>↑</button>
                  <button style={miniBtn} onClick={() => moveLens(idx, 1)} disabled={idx === lensDrafts.length - 1}>↓</button>
                  <input
                    value={lens.label}
                    onChange={(e) => setLensDrafts((prev) => prev.map((x, i) => i === idx ? { ...x, label: e.target.value } : x))}
                    style={{ ...editorInput, flex: 1 }}
                  />
                  <label style={{ color: "#94a3b8", fontSize: 10, display: "flex", alignItems: "center", gap: 4 }}>
                    <input
                      type="checkbox"
                      checked={!lens.hidden}
                      onChange={(e) => setLensDrafts((prev) => prev.map((x, i) => i === idx ? { ...x, hidden: !e.target.checked } : x))}
                    />
                    visible
                  </label>
                </div>

                <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8 }}>
                  <label style={editorLabel}>
                    <span>Period</span>
                    <input
                      value={lens.period}
                      onChange={(e) => setLensDrafts((prev) => prev.map((x, i) => i === idx ? { ...x, period: e.target.value } : x))}
                      style={editorInput}
                    />
                  </label>
                  <label style={editorLabel}>
                    <span>As Of</span>
                    <input
                      value={lens.asOf}
                      onChange={(e) => setLensDrafts((prev) => prev.map((x, i) => i === idx ? { ...x, asOf: e.target.value } : x))}
                      style={editorInput}
                    />
                  </label>
                </div>

                <label style={{ color: "#94a3b8", fontSize: 10, display: "flex", alignItems: "center", gap: 6 }}>
                  <input
                    type="checkbox"
                    checked={lens.thresholdEnabled}
                    onChange={(e) => setLensDrafts((prev) => prev.map((x, i) => i === idx ? { ...x, thresholdEnabled: e.target.checked } : x))}
                  />
                  threshold health
                </label>

                {lens.thresholdEnabled && (
                  <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8 }}>
                    <label style={editorLabel}>
                      <span>green_lt</span>
                      <input
                        value={lens.greenLt}
                        onChange={(e) => setLensDrafts((prev) => prev.map((x, i) => i === idx ? { ...x, greenLt: e.target.value } : x))}
                        style={editorInput}
                      />
                    </label>
                    <label style={editorLabel}>
                      <span>amber_lt</span>
                      <input
                        value={lens.amberLt}
                        onChange={(e) => setLensDrafts((prev) => prev.map((x, i) => i === idx ? { ...x, amberLt: e.target.value } : x))}
                        style={editorInput}
                      />
                    </label>
                    <label style={editorLabel}>
                      <span>amber_gte</span>
                      <input
                        value={lens.amberGte}
                        onChange={(e) => setLensDrafts((prev) => prev.map((x, i) => i === idx ? { ...x, amberGte: e.target.value } : x))}
                        style={editorInput}
                      />
                    </label>
                    <label style={editorLabel}>
                      <span>red_gte</span>
                      <input
                        value={lens.redGte}
                        onChange={(e) => setLensDrafts((prev) => prev.map((x, i) => i === idx ? { ...x, redGte: e.target.value } : x))}
                        style={editorInput}
                      />
                    </label>
                  </div>
                )}
              </div>
            ))}
          </div>

          <div style={{ display: "flex", gap: 8, justifyContent: "space-between", marginTop: 4 }}>
            <button onClick={() => void restoreDefaults()} style={ctrlBtn} disabled={editSaving}>
              Restore Defaults
            </button>
            <div style={{ display: "flex", gap: 8 }}>
              <button onClick={() => setEditOpen(false)} style={ctrlBtn} disabled={editSaving}>Cancel</button>
              <button onClick={() => void saveEditor()} style={ctrlBtn} disabled={editSaving}>
                {editSaving ? "Saving…" : "Save"}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* states */}
      {loading && !summary && (
        <Centered>Generating brain map…</Centered>
      )}
      {error && (
        <Centered>
          <div style={{ color: "#f87171", maxWidth: 420, textAlign: "center" }}>{error}</div>
        </Centered>
      )}
      {!loading && !error && summary && summary.categories.length === 0 && (
        <Centered>No categories — index some documents first.</Centered>
      )}
    </div>
  );
}

function summaryFromResolution(
  resolved: SummaryResolution,
  labels?: Record<string, string>,
): Summary {
  const categories: Category[] = [];
  for (const lens of resolved.lenses) {
    const members = lens.selected_node_ids ?? [];
    const headline = members[0] ?? lens.lens_id;
    categories.push({
      label: lens.label,
      headline_node_id: headline,
      member_node_ids: members,
    });
  }
  return {
    categories,
    source: resolved.source,
    labels: labels ?? {},
  };
}

function measureSubLabel(measure: MeasureResolution): string {
  if (measure.state === "unknown") return "unknown";
  const value = typeof measure.value === "number" ? measure.value : null;
  if (value === null) return "unknown";

  const unit = (measure.unit ?? "").toLowerCase();
  if (unit === "percent") return `${(value * 100).toFixed(1)}%`;
  if (unit === "count") return formatCompact(value);
  return `${formatCompact(value)}${measure.unit ? ` ${measure.unit}` : ""}`;
}

function formatCompact(n: number): string {
  const abs = Math.abs(n);
  if (abs >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(1)}B`;
  if (abs >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (abs >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  if (Number.isInteger(n)) return String(n);
  return n.toFixed(2);
}

function colorForHealth(state: HealthResolution["state"]): string {
  if (state === "good") return "#16a34a";
  if (state === "watch") return "#d97706";
  if (state === "at_risk") return "#dc2626";
  return "#64748b";
}

function topLevelSubLabel(lens: LensResolution): string {
  const measure = measureSubLabel(lens.measure);
  const health = healthSubLabel(lens.health.state);
  if (measure === health) return measure;
  return `${measure} · ${health}`;
}

function healthSubLabel(state: HealthResolution["state"]): string {
  if (state === "good") return "good";
  if (state === "watch") return "watch";
  if (state === "at_risk") return "at risk";
  return "unknown";
}

function topLevelTooltip(label: string, lens: LensResolution): string {
  const lines = [
    label,
    `measure: ${measureSubLabel(lens.measure)}`,
    `health: ${healthSubLabel(lens.health.state)}`,
  ];
  if (lens.measure.explanation) lines.push(`measure note: ${lens.measure.explanation}`);
  if (lens.health.explanation) lines.push(`health note: ${lens.health.explanation}`);
  return lines.join("\n");
}

function prettyJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function PanelBlock({ title, body }: { title: string; body: string }) {
  return (
    <div style={{ marginTop: 8 }}>
      <div style={{ color: "#94a3b8", fontSize: 10, marginBottom: 4 }}>{title}</div>
      <pre style={{
        margin: 0,
        background: "#020617",
        border: "1px solid #1e293b",
        borderRadius: 6,
        padding: 8,
        color: "#cbd5e1",
        fontSize: 10,
        lineHeight: 1.35,
        whiteSpace: "pre-wrap",
        wordBreak: "break-word",
      }}>{body}</pre>
    </div>
  );
}

function renderEvidenceLineage(items: EvidenceLineageItem[]): string {
  if (!items || items.length === 0) return "none";
  return items
    .map((item, i) => {
      const lines = [
        `${i + 1}. ${item.source_node_name} (${item.source_type})`,
        `evidence_id: ${item.evidence_id}`,
      ];
      if (item.source_path) lines.push(`source_path: ${item.source_path}`);
      if (item.row_ref) lines.push(`row_ref: ${item.row_ref}`);
      if (item.timestamp) lines.push(`timestamp: ${item.timestamp}`);
      if (item.quote) lines.push(`quote: ${item.quote}`);
      return lines.join("\n");
    })
    .join("\n\n");
}

function renderLineageGaps(gaps: LineageGap[]): string {
  if (!gaps || gaps.length === 0) return "none";
  return gaps.map((g, i) => `${i + 1}. [${g.code}] ${g.message}`).join("\n");
}

function CircleNode({ x, y, size, label, sub, title, bg, border, bold, onClick }: {
  x: number; y: number; size: number; label: string; sub?: string;
  title?: string;
  bg: string; border: string; bold?: boolean; onClick?: () => void;
}) {
  return (
    <div
      onClick={onClick}
      title={title}
      style={{
        position: "absolute", left: x, top: y, width: size, height: size,
        transform: "translate(-50%, -50%)", borderRadius: "50%",
        background: bg, border: `2px solid ${border}`,
        display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center",
        padding: 8, boxSizing: "border-box", textAlign: "center",
        cursor: onClick ? "pointer" : "default",
        boxShadow: "0 4px 14px rgba(0,0,0,0.35)",
      }}
    >
      <span style={{
        color: "#f8fafc", fontSize: bold ? 14 : 12, fontWeight: bold ? 700 : 600, lineHeight: 1.15,
        display: "-webkit-box", WebkitLineClamp: 3, WebkitBoxOrient: "vertical", overflow: "hidden",
      }}>{label}</span>
      {sub && <span style={{ color: "rgba(248,250,252,0.7)", fontSize: 10, marginTop: 3 }}>{sub}</span>}
    </div>
  );
}

function Centered({ children }: { children: React.ReactNode }) {
  return (
    <div style={{ position: "absolute", inset: 0, display: "flex", alignItems: "center", justifyContent: "center", color: "#64748b", fontSize: 14, pointerEvents: "none" }}>
      {children}
    </div>
  );
}

const ctrlBtn: React.CSSProperties = {
  padding: "5px 12px", borderRadius: 8, border: "1px solid #334155",
  background: "#1e293b", color: "#94a3b8", fontSize: 12, cursor: "pointer",
};

const crumbBtn: React.CSSProperties = {
  border: "none",
  background: "transparent",
  color: "#94a3b8",
  fontSize: 11,
  cursor: "pointer",
  padding: 0,
};

const miniBtn: React.CSSProperties = {
  fontSize: 10,
  padding: "2px 7px",
  borderRadius: 5,
  border: "1px solid #334155",
  background: "#0f172a",
  color: "#cbd5e1",
};

const editorLabel: React.CSSProperties = {
  color: "#94a3b8",
  fontSize: 10,
  display: "flex",
  flexDirection: "column",
  gap: 4,
};

const editorInput: React.CSSProperties = {
  border: "1px solid #334155",
  background: "#0f172a",
  color: "#e2e8f0",
  borderRadius: 6,
  padding: "4px 6px",
  fontSize: 11,
};
