import { describe, it, expectTypeOf } from 'vitest'
import type { Node, Edge, EngineEvent, FocusKpiOutput, DocumentSummary, DocumentVersion } from './index'
import type { AxonMindTransport } from './transport'

// These are compile-time checks: they fail if a required field is removed or
// a discriminated union variant loses its discriminant. The `satisfies` keyword
// means TypeScript errors here before the test even runs.

describe('@axonmind/types shape checks', () => {
  it('Node has all required fields', () => {
    const n = {
      id: 'kpi.rev', kind: 'Kpi' as const, name: 'Revenue',
      created_at: '2024-01-01T00:00:00Z', updated_at: '2024-01-01T00:00:00Z',
      attrs: {}, confidence: 0.8, is_tainted: false, requires_human_review: false,
    } satisfies Node
    expectTypeOf(n).toMatchTypeOf<Node>()
  })

  it('Edge evidence field is an array of IDs, not objects', () => {
    const e = {
      id: 'e1', from: 'a', to: 'b', kind: 'Influences' as const,
      evidence: ['ev-1', 'ev-2'],
      confidence: 0.8, created_at: '', created_by: 'Rule' as const,
      is_tainted: false, requires_human_review: false,
    } satisfies Edge
    expectTypeOf(e.evidence).toEqualTypeOf<string[]>()
  })

  it('EngineEvent is a discriminated union on type', () => {
    const upserted = { type: 'node_upserted', node_id: 'kpi.rev' } satisfies EngineEvent
    const rebuilt  = { type: 'cache_rebuilt' } satisfies EngineEvent
    const batchStarted = { type: 'ingest_batch_started', job_id: 'job-1', root_path: '/tmp/docs', total_files: 3 } satisfies EngineEvent
    const filePhase = { type: 'ingest_file_phase', job_id: 'job-1', path: '/tmp/docs/a.pdf', phase: 'parsing' } satisfies EngineEvent
    expectTypeOf(upserted).toMatchTypeOf<EngineEvent>()
    expectTypeOf(rebuilt).toMatchTypeOf<EngineEvent>()
    expectTypeOf(batchStarted).toMatchTypeOf<EngineEvent>()
    expectTypeOf(filePhase).toMatchTypeOf<EngineEvent>()
  })

  it('AxonMindTransport focusKpi returns FocusKpiOutput', () => {
    expectTypeOf<ReturnType<AxonMindTransport['focusKpi']>>()
      .toEqualTypeOf<Promise<FocusKpiOutput>>()
  })

  // Phase 0 (document versioning): the Library's version timeline depends on these fields
  // mirroring Rust exactly. If the Rust DocumentSummary loses a versioning field, the
  // `satisfies` check below fails to compile — catching drift before runtime.
  it('DocumentSummary carries logical-doc + version metadata', () => {
    const d = {
      node_id: 'doc.abc', name: 'Policy.pdf', source_path: null, sha256: null,
      indexed_at: 0, concept_count: 0, evidence_count: 0,
      logical_doc_id: 'ldoc.x', version_no: 1, version_count: 1,
    } satisfies DocumentSummary
    expectTypeOf(d.version_count).toEqualTypeOf<number>()
  })

  it('DocumentVersion has the lineage fields the diff UI reads', () => {
    const v = {
      logical_doc_id: 'ldoc.x', version_no: 2, node_id: 'doc.def', sha256: 'deadbeef',
      structural_sha256: null, indexed_at: 0, source_path: null,
      previous_node_id: 'doc.abc', superseded: false,
    } satisfies DocumentVersion
    expectTypeOf(v.superseded).toMatchTypeOf<boolean>()
  })

  it('AxonMindTransport exposes version timeline + content retrieval', () => {
    expectTypeOf<ReturnType<AxonMindTransport['listDocumentVersions']>>()
      .toEqualTypeOf<Promise<DocumentVersion[]>>()
    expectTypeOf<ReturnType<AxonMindTransport['getDocumentContent']>>()
      .toEqualTypeOf<Promise<string>>()
  })
})
