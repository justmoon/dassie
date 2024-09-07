import { describe, test, vi } from "vitest"

import {
  type ComputationContext,
  createComputed,
  createReactor,
  createSignal,
} from ".."

describe("createComputed", () => {
  test("should create a computed", ({ expect }) => {
    const reactor = createReactor()
    const computed = createComputed(reactor, () => 1)
    expect(computed).toBeTypeOf("object")
  })

  test("should be able to access computed value", ({ expect }) => {
    const reactor = createReactor()
    const computed = createComputed(reactor, () => 1)
    expect(computed.read()).toBe(1)
  })

  test("computed value should stay up-to-date", ({ expect }) => {
    const reactor = createReactor()
    const signal = createSignal(1)
    const computed = createComputed(
      reactor,
      (sig) => sig.readAndTrack(signal) * 3,
    )
    expect(computed.read()).toBe(3)
    signal.write(5)
    expect(computed.read()).toBe(15)
  })

  test("computed value should only update when dependencies change", ({
    expect,
  }) => {
    const reactor = createReactor()
    const signal = createSignal(1)
    const computation = vi.fn(
      (sig: ComputationContext) => sig.readAndTrack(signal) * 3,
    )
    const computed = createComputed(reactor, computation)

    expect(computation).toHaveBeenCalledTimes(0)

    expect(computed.read()).toBe(3)
    expect(computation).toHaveBeenCalledTimes(1)

    signal.write(2)
    expect(computed.read()).toBe(6)
    expect(computation).toHaveBeenCalledTimes(2)

    signal.write(2)
    expect(computed.read()).toBe(6)
    expect(computation).toHaveBeenCalledTimes(2)
  })

  test("diamond dependencies should update correctly", ({ expect }) => {
    const reactor = createReactor()
    const a = createSignal(1)

    const b1Computation = vi.fn(
      (sig: ComputationContext) => sig.readAndTrack(a) * 2,
    )
    const b1 = createComputed(reactor, b1Computation)

    const b2Computation = vi.fn(
      (sig: ComputationContext) => sig.readAndTrack(a) * 3,
    )
    const b2 = createComputed(reactor, b2Computation)

    const c1Computation = vi.fn(
      (sig: ComputationContext) => sig.readAndTrack(b1) + 8,
    )
    const c1 = createComputed(reactor, c1Computation)

    const c2Computation = vi.fn(
      (sig: ComputationContext) => sig.readAndTrack(b2) + 4,
    )
    const c2 = createComputed(reactor, c2Computation)

    const dComputation = vi.fn(
      (sig: ComputationContext) => sig.readAndTrack(c1) - sig.readAndTrack(c2),
    )
    const d = createComputed(reactor, dComputation)

    expect(b1Computation).toHaveBeenCalledTimes(0)
    expect(b2Computation).toHaveBeenCalledTimes(0)
    expect(c1Computation).toHaveBeenCalledTimes(0)
    expect(c2Computation).toHaveBeenCalledTimes(0)
    expect(dComputation).toHaveBeenCalledTimes(0)

    expect(d.read()).toBe(3)
    expect(b1Computation).toHaveBeenCalledTimes(1)
    expect(b2Computation).toHaveBeenCalledTimes(1)
    expect(c1Computation).toHaveBeenCalledTimes(1)
    expect(c2Computation).toHaveBeenCalledTimes(1)
    expect(dComputation).toHaveBeenCalledTimes(1)

    a.write(3)
    expect(b1Computation).toHaveBeenCalledTimes(1)
    expect(b2Computation).toHaveBeenCalledTimes(1)
    expect(c1Computation).toHaveBeenCalledTimes(1)
    expect(c2Computation).toHaveBeenCalledTimes(1)
    expect(dComputation).toHaveBeenCalledTimes(1)

    expect(d.read()).toBe(1)
    expect(b1Computation).toHaveBeenCalledTimes(2)
    expect(b2Computation).toHaveBeenCalledTimes(2)
    expect(c1Computation).toHaveBeenCalledTimes(2)
    expect(c2Computation).toHaveBeenCalledTimes(2)
    expect(dComputation).toHaveBeenCalledTimes(2)
  })

  test("should call onCleanup handler if reactor is disposed", async ({
    expect,
  }) => {
    const reactor = createReactor()
    const signal = createSignal(1)

    const cleanup = vi.fn(() => {})

    const computation = vi.fn((sig: ComputationContext) => {
      sig.onCleanup(cleanup)
      return sig.readAndTrack(signal)
    })

    const computed = createComputed(reactor, computation)

    expect(computation).toHaveBeenCalledTimes(0)
    expect(cleanup).toHaveBeenCalledTimes(0)

    expect(computed.read()).toBe(1)
    expect(computation).toHaveBeenCalledTimes(1)
    expect(cleanup).toHaveBeenCalledTimes(0)

    await reactor.dispose()
    expect(cleanup).toHaveBeenCalledTimes(1)
  })

  test("should not call scope.onCleanup handler even if computed re-runs", ({
    expect,
  }) => {
    const reactor = createReactor()
    const signal = createSignal(1)

    const cleanup = vi.fn(() => {})

    const computation = vi.fn((sig: ComputationContext) => {
      sig.onCleanup(cleanup)
      return sig.readAndTrack(signal)
    })

    const computed = createComputed(reactor, computation)

    expect(computation).toHaveBeenCalledTimes(0)
    expect(cleanup).toHaveBeenCalledTimes(0)

    expect(computed.read()).toBe(1)
    expect(computation).toHaveBeenCalledTimes(1)
    expect(cleanup).toHaveBeenCalledTimes(0)

    signal.write(2)
    expect(computed.read()).toBe(2)
    expect(computation).toHaveBeenCalledTimes(2)
    expect(cleanup).toHaveBeenCalledTimes(0)
  })

  test("should only update when selected keys change", ({ expect }) => {
    const reactor = createReactor()
    const signal = createSignal({ a: 1, b: 2 })
    const computation = vi.fn((sig: ComputationContext) => {
      const { a } = sig.readKeysAndTrack(signal, ["a"])
      return a
    })
    const computed = createComputed(reactor, computation)

    expect(computation).toHaveBeenCalledTimes(0)

    expect(computed.read()).toBe(1)
    expect(computation).toHaveBeenCalledTimes(1)

    signal.write({ a: 1, b: 2 })
    expect(computed.read()).toBe(1)
    expect(computation).toHaveBeenCalledTimes(1)

    signal.write({ a: 2, b: 2 })
    expect(computed.read()).toBe(2)
    expect(computation).toHaveBeenCalledTimes(2)
  })
})
