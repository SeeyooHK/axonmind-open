import { describe, it, expect, vi } from 'vitest'
import { renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import type { AxonMindTransport, GraphSearchOutput } from '@axonmind/types'
import { AxonMindProvider } from '../context'
import { useGraphSearch } from './useGraphSearch'

function makeTransport(overrides: Partial<AxonMindTransport> = {}): AxonMindTransport {
  const stub = (): Promise<never> => Promise.reject(new Error('not implemented'))
  return {
    focusKpi: stub, explainKpi: stub, getEvidence: stub, impactRadius: stub,
    traceDecision: stub, suggestActions: stub, graphSearch: stub, reasoningSearch: stub, exportJson: stub,
    suggestSummary: stub, resolveBrainMapDefaultSummary: stub, resolveBrainMapLensChildren: stub,
    getBrainMapDefaultConfig: stub, updateBrainMapDefaultConfig: stub, restoreBrainMapDefaultConfig: stub,
    listDocuments: stub, removeDocument: stub, regenerateDocument: stub,
    indexPath: stub, indexMarkdown: stub,
    createGenerationFromPaths: stub, listGenerations: stub, exportGeneration: stub,
    ...overrides,
  }
}

function makeWrapper(transport: AxonMindTransport) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return <AxonMindProvider transport={transport}>{children}</AxonMindProvider>
  }
}

describe('useGraphSearch', () => {
  it('empty string → stays idle without calling transport', () => {
    // WHY: the backend sanitize_fts_query returns "" for empty input, which the
    // store short-circuits; mirroring this guard here avoids a redundant round-trip.
    const graphSearch = vi.fn()
    const { result } = renderHook(() => useGraphSearch(''), {
      wrapper: makeWrapper(makeTransport({ graphSearch })),
    })
    expect(result.current.loading).toBe(false)
    expect(result.current.data).toBeNull()
    expect(graphSearch).not.toHaveBeenCalled()
  })

  it('whitespace-only string → stays idle', () => {
    // WHY: " ".trim() === "" — whitespace queries would hit the backend and return
    // zero results, wasting a round-trip and flickering the loading state.
    const graphSearch = vi.fn()
    const { result } = renderHook(() => useGraphSearch('   '), {
      wrapper: makeWrapper(makeTransport({ graphSearch })),
    })
    expect(result.current.loading).toBe(false)
    expect(graphSearch).not.toHaveBeenCalled()
  })

  it('valid query → sets loading then resolves data', async () => {
    const output: GraphSearchOutput = { nodes: [], matched_via: [] }
    const graphSearch = vi.fn().mockResolvedValue(output)
    const { result } = renderHook(() => useGraphSearch('revenue'), {
      wrapper: makeWrapper(makeTransport({ graphSearch })),
    })
    expect(result.current.loading).toBe(true)
    await waitFor(() => expect(result.current.loading).toBe(false))
    expect(result.current.data).toEqual(output)
    expect(graphSearch).toHaveBeenCalledWith({
      query: 'revenue', kinds: undefined, limit: undefined,
    })
  })

  it('transport error → surfaces error string', async () => {
    const graphSearch = vi.fn().mockRejectedValue(new Error('fts error'))
    const { result } = renderHook(() => useGraphSearch('revenue'), {
      wrapper: makeWrapper(makeTransport({ graphSearch })),
    })
    await waitFor(() => expect(result.current.loading).toBe(false))
    expect(result.current.error).toBe('Error: fts error')
    expect(result.current.data).toBeNull()
  })
})
