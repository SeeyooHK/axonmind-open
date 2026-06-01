import { describe, it, expect, vi } from 'vitest'
import { renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import type { AxonMindTransport, FocusKpiOutput, Node } from '@axonmind/types'
import { AxonMindProvider } from '../context'
import { useFocusKpi } from './useFocusKpi'

function makeTransport(overrides: Partial<AxonMindTransport> = {}): AxonMindTransport {
  const stub = (): Promise<never> => Promise.reject(new Error('not implemented'))
  return {
    focusKpi: stub, explainKpi: stub, getEvidence: stub, impactRadius: stub,
    traceDecision: stub, suggestActions: stub, graphSearch: stub, exportJson: stub,
    suggestSummary: stub, resolveBrainMapDefaultSummary: stub, resolveBrainMapLensChildren: stub,
    getBrainMapDefaultConfig: stub, updateBrainMapDefaultConfig: stub, restoreBrainMapDefaultConfig: stub,
    listDocuments: stub, removeDocument: stub, regenerateDocument: stub,
    indexPath: stub, indexMarkdown: stub,
    createGenerationFromPaths: stub, listGenerations: stub, exportGeneration: stub,
    ...overrides,
  }
}

function makeNode(id: string): Node {
  return {
    id, kind: 'Kpi', name: id, created_at: '', updated_at: '',
    attrs: {}, confidence: 0.8, is_tainted: false, requires_human_review: false,
  }
}

function makeWrapper(transport: AxonMindTransport) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return <AxonMindProvider transport={transport}>{children}</AxonMindProvider>
  }
}

const emptyOutput: FocusKpiOutput = {
  kpi: makeNode('kpi.rev'), drivers: [], blockers: [],
  risks: [], owner: null, evidence_count: 0,
}

describe('useFocusKpi', () => {
  it('null kpiId → stays idle without calling transport', () => {
    // WHY: a null kpiId means "nothing selected"; calling transport would waste a
    // round-trip and overwrite state the caller did not request.
    const focusKpi = vi.fn()
    const { result } = renderHook(() => useFocusKpi(null), {
      wrapper: makeWrapper(makeTransport({ focusKpi })),
    })
    expect(result.current.loading).toBe(false)
    expect(result.current.data).toBeNull()
    expect(result.current.error).toBeNull()
    expect(focusKpi).not.toHaveBeenCalled()
  })

  it('valid kpiId → sets loading then resolves data', async () => {
    // WHY: the loading flag drives skeleton UI; if it is never set, users see a blank
    // panel during the fetch with no indication that data is coming.
    const focusKpi = vi.fn().mockResolvedValue(emptyOutput)
    const { result } = renderHook(() => useFocusKpi('kpi.rev'), {
      wrapper: makeWrapper(makeTransport({ focusKpi })),
    })
    expect(result.current.loading).toBe(true)
    await waitFor(() => expect(result.current.loading).toBe(false))
    expect(result.current.data).toEqual(emptyOutput)
    expect(result.current.error).toBeNull()
    expect(focusKpi).toHaveBeenCalledWith({ kpi_id: 'kpi.rev' })
  })

  it('transport error → surfaces error string, clears data', async () => {
    // WHY: an unhandled rejection silently leaves the hook in the loading state;
    // the error field is the only signal the UI has to render an error message.
    const focusKpi = vi.fn().mockRejectedValue(new Error('network failure'))
    const { result } = renderHook(() => useFocusKpi('kpi.rev'), {
      wrapper: makeWrapper(makeTransport({ focusKpi })),
    })
    await waitFor(() => expect(result.current.loading).toBe(false))
    expect(result.current.error).toBe('Error: network failure')
    expect(result.current.data).toBeNull()
  })

  it('unmount before resolution does not update state', async () => {
    // WHY: without the cancelled flag, setState is called on an unmounted component.
    // React logs a warning and may overwrite state in a subsequent mount of the same
    // component, surfacing stale data.
    let resolve!: (v: FocusKpiOutput) => void
    const focusKpi = vi.fn().mockReturnValue(new Promise(r => { resolve = r }))
    const { result, unmount } = renderHook(() => useFocusKpi('kpi.rev'), {
      wrapper: makeWrapper(makeTransport({ focusKpi })),
    })
    unmount()
    resolve(emptyOutput)
    await new Promise(r => setTimeout(r, 0))
    expect(result.current.data).toBeNull()
  })
})
