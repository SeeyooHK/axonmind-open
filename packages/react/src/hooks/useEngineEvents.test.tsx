import { describe, it, expect, vi } from 'vitest'
import { renderHook } from '@testing-library/react'
import type { ReactNode } from 'react'
import type { AxonMindTransport, EngineEvent } from '@axonmind/types'
import { AxonMindProvider } from '../context'
import { useEngineEvents } from './useEngineEvents'

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

describe('useEngineEvents', () => {
  it('subscribes on mount and routes events to handler', () => {
    // WHY: if onEvent is never called the UI never updates on engine events
    // and users must manually refresh to see graph changes.
    let emit!: (e: EngineEvent) => void
    const onEvent = vi.fn((h: (e: EngineEvent) => void) => { emit = h; return () => {} })
    const handler = vi.fn()

    renderHook(() => useEngineEvents(handler), {
      wrapper: makeWrapper(makeTransport({ onEvent })),
    })

    expect(onEvent).toHaveBeenCalledOnce()
    emit({ type: 'cache_rebuilt' })
    expect(handler).toHaveBeenCalledWith({ type: 'cache_rebuilt' })
  })

  it('calls unsubscribe returned by onEvent when component unmounts', () => {
    // WHY: a leaked subscription receives events after the component is gone,
    // calling setState on an unmounted tree and causing memory leaks.
    const unsubscribe = vi.fn()
    const onEvent = vi.fn(() => unsubscribe)

    const { unmount } = renderHook(() => useEngineEvents(vi.fn()), {
      wrapper: makeWrapper(makeTransport({ onEvent })),
    })
    unmount()
    expect(unsubscribe).toHaveBeenCalledOnce()
  })

  it('handler ref is updated without re-subscribing when handler identity changes', () => {
    // WHY: callers commonly pass inline arrow functions; re-subscribing on every
    // render would cause an unsubscribe/subscribe churn that drops events.
    let emit!: (e: EngineEvent) => void
    const onEvent = vi.fn((h: (e: EngineEvent) => void) => { emit = h; return () => {} })

    const handler1 = vi.fn()
    const handler2 = vi.fn()
    let currentHandler = handler1

    const { rerender } = renderHook(() => useEngineEvents(currentHandler), {
      wrapper: makeWrapper(makeTransport({ onEvent })),
    })

    currentHandler = handler2
    rerender()

    // onEvent must still have been called only once (no re-subscribe)
    expect(onEvent).toHaveBeenCalledOnce()
    // but the new handler receives the event
    emit({ type: 'cache_rebuilt' })
    expect(handler1).not.toHaveBeenCalled()
    expect(handler2).toHaveBeenCalledWith({ type: 'cache_rebuilt' })
  })

  it('does not throw when transport.onEvent is absent', () => {
    // WHY: onEvent is optional; non-Tauri transports (e.g. HTTP polling) may not
    // implement it, and the hook must degrade silently rather than crash.
    expect(() =>
      renderHook(() => useEngineEvents(vi.fn()), {
        wrapper: makeWrapper(makeTransport()),
      })
    ).not.toThrow()
  })
})
