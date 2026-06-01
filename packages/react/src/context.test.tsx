import { describe, it, expect, vi } from 'vitest'
import { renderHook } from '@testing-library/react'
import { useAxonMind } from './context'

describe('useAxonMind', () => {
  it('throws a clear error when used outside AxonMindProvider', () => {
    // WHY: returning null or undefined silently would cause downstream hooks to crash
    // with confusing "cannot read property of undefined" errors rather than a clear
    // "missing provider" message that points the developer to the root cause.
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    expect(() => renderHook(() => useAxonMind()))
      .toThrow('useAxonMind must be used inside <AxonMindProvider>')
    spy.mockRestore()
  })
})
