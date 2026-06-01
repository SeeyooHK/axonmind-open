import { describe, it, expect } from 'vitest'
import type { GraphExportV1, Node, Edge, EdgeKind } from '@axonmind/types'
import { toGraphElements } from './adapter'

function node(id: string): Node {
  return {
    id,
    kind: 'Customer',
    name: id,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    attrs: {},
    confidence: 0.5,
    is_tainted: false,
    requires_human_review: false,
  }
}

function edge(id: string, from: string, to: string, kind: EdgeKind): Edge {
  return {
    id,
    from,
    to,
    kind,
    evidence: ['ev1'],
    confidence: 0.5,
    created_by: 'Llm',
    created_at: '2026-01-01T00:00:00Z',
    is_tainted: false,
    requires_human_review: false,
  }
}

function graph(edges: Edge[]): GraphExportV1 {
  return {
    schema_version: 1,
    exported_at: '2026-01-01T00:00:00Z',
    workspace_id: 'test',
    nodes: [node('doc.1'), node('customer.acme'), node('product.x')],
    edges,
    evidence: [],
    edge_evidence: [],
    metric_values: [],
    kpi_candidates: [],
  }
}

describe('toGraphElements', () => {
  it('omits MentionedIn provenance edges by default but keeps business relations', () => {
    // Why: MentionedIn is document→concept provenance, not a relationship between concepts.
    // Leaving it in makes every concept a spoke off a document hub — the hairball we want gone.
    const g = graph([
      edge('e1', 'doc.1', 'customer.acme', 'MentionedIn'),
      edge('e2', 'customer.acme', 'product.x', 'DependsOn'),
    ])

    const { edges } = toGraphElements(g)

    expect(edges.map(e => e.id)).toEqual(['e2'])
    expect(edges.every(e => e.kind !== 'MentionedIn')).toBe(true)
  })

  it('renders every edge kind when hideEdgeKinds is empty', () => {
    // The filter is view-only policy, not data loss: callers can opt back into the full graph.
    const g = graph([
      edge('e1', 'doc.1', 'customer.acme', 'MentionedIn'),
      edge('e2', 'customer.acme', 'product.x', 'DependsOn'),
    ])

    const { edges } = toGraphElements(g, { hideEdgeKinds: [] })

    expect(edges.map(e => e.id).sort()).toEqual(['e1', 'e2'])
  })
})
