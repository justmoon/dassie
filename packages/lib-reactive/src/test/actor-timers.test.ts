import { afterEach, beforeEach, describe, test, vi } from "vitest"

import { createActor, createReactor } from ".."

describe("createActor timers", () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })
  test("should run a timeout after the specified time", ({ expect }) => {
    const timeout = vi.fn()

    const actor = () =>
      createActor((sig) => {
        sig.timeout(timeout, 100)
      })

    createReactor(actor)

    vi.advanceTimersByTime(99)
    expect(timeout).toHaveBeenCalledTimes(0)

    vi.advanceTimersByTime(1)
    expect(timeout).toHaveBeenCalledTimes(1)
  })

  test("should not run a timeout if the reactor is disposed before the timeout is reached", async ({
    expect,
  }) => {
    const timeout = vi.fn()

    const actor = () =>
      createActor((sig) => {
        sig.timeout(timeout, 100)
      })

    const reactor = createReactor(actor)

    vi.advanceTimersByTime(99)
    await reactor.dispose()

    vi.advanceTimersByTime(1)
    expect(timeout).toHaveBeenCalledTimes(0)
  })

  test("should run an interval until the reactor is disposed", async ({
    expect,
  }) => {
    const interval = vi.fn()

    const actor = () =>
      createActor((sig) => {
        sig.interval(interval, 100)
      })

    const reactor = createReactor(actor)

    vi.advanceTimersByTime(99)
    expect(interval).toHaveBeenCalledTimes(0)

    vi.advanceTimersByTime(1)
    expect(interval).toHaveBeenCalledTimes(1)

    vi.advanceTimersByTime(99)
    expect(interval).toHaveBeenCalledTimes(1)

    vi.advanceTimersByTime(1)
    expect(interval).toHaveBeenCalledTimes(2)

    await reactor.dispose()

    vi.advanceTimersByTime(100)
    expect(interval).toHaveBeenCalledTimes(2)
  })
})
