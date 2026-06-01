import { describe, it, expectTypeOf } from 'vitest'
import type { Node, Edge, EngineEvent, FocusKpiOutput } from './index'
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
    expectTypeOf(upserted).toMatchTypeOf<EngineEvent>()
    expectTypeOf(rebuilt).toMatchTypeOf<EngineEvent>()
  })

  it('AxonMindTransport focusKpi returns FocusKpiOutput', () => {
    expectTypeOf<ReturnType<AxonMindTransport['focusKpi']>>()
      .toEqualTypeOf<Promise<FocusKpiOutput>>()
  })
})
