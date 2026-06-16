// Compile-time drift guard.
//
// The hand-maintained types in `@axonmind/types` are an intentional ergonomic curation
// of the ts-rs source of truth in `crates/*/bindings/` (e.g. Rust `Option<T>` is rendered
// as `T | null` by ts-rs but exposed here as an optional `?:` field). Those I/O types are
// deliberately NOT identical, so they are not guarded.
//
// But enums and pure-data mirror types have NO such divergence — they MUST match the Rust
// source exactly. A missing variant (e.g. a new `NodeKind` added in Rust but forgotten here)
// is a silent bug. This file fails `tsc` if any guarded type drifts from its generated binding.
//
// When you add a new enum or pure-data mirror type, add it below.
// Dev-only — excluded from the published package via "files": ["src"] in package.json.

import type { NodeKind, EdgeKind, ExtractorKind, KpiStatus, KpiTrend } from "./src/index";
import type { NodeKindCount, SearchMatchKind } from "./src/transport";

import type { NodeKind as GenNodeKind } from "../../crates/axonmind_core/bindings/NodeKind";
import type { EdgeKind as GenEdgeKind } from "../../crates/axonmind_core/bindings/EdgeKind";
import type { ExtractorKind as GenExtractorKind } from "../../crates/axonmind_core/bindings/ExtractorKind";
import type { KpiStatus as GenKpiStatus } from "../../crates/axonmind_core/bindings/KpiStatus";
import type { KpiTrend as GenKpiTrend } from "../../crates/axonmind_core/bindings/KpiTrend";
import type { NodeKindCount as GenNodeKindCount } from "../../crates/axonmind_engine/bindings/NodeKindCount";
import type { SearchMatchKind as GenSearchMatchKind } from "../../crates/axonmind_engine/bindings/SearchMatchKind";

/** `true` iff A and B are mutually assignable (identical for closed unions / pure-data shapes). */
type Exact<A, B> = [A] extends [B] ? ([B] extends [A] ? true : never) : never;

// Each slot must be `true`. If a type drifts, its `Exact<...>` becomes `never` and `true`
// is no longer assignable to it — `tsc` fails at that exact position.
export const _conformance: [
  Exact<NodeKind, GenNodeKind>,
  Exact<EdgeKind, GenEdgeKind>,
  Exact<ExtractorKind, GenExtractorKind>,
  Exact<KpiStatus, GenKpiStatus>,
  Exact<KpiTrend, GenKpiTrend>,
  Exact<NodeKindCount, GenNodeKindCount>,
  Exact<SearchMatchKind, GenSearchMatchKind>,
] = [true, true, true, true, true, true, true];
