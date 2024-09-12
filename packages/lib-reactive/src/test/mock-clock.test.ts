import { describe, test, vi } from "vitest"

import { MockClockImplementation } from "../mocks/clock"

describe("MockClock", () => {
  describe("now", () => {
    test("should return the current time", ({ expect }) => {
      const clock = new MockClockImplementation(1000)
      expect(clock.now()).toBe(1000)
    })
  })

  describe("tick", () => {
    test("should advance time", async ({ expect }) => {
      const clock = new MockClockImplementation(1000)
      await clock.tick(100)
      expect(clock.now()).toBe(1100)
    })

    test("should call timeout callback", async ({ expect }) => {
      const clock = new MockClockImplementation()
      const callback = vi.fn()

      clock.setTimeout(callback, 100)

      await clock.tick(200)

      expect(callback).toHaveBeenCalledTimes(1)
    })

    test("should not call timeout that hasn't been reached", async ({
      expect,
    }) => {
      const clock = new MockClockImplementation()
      const callback = vi.fn()

      clock.setTimeout(callback, 100)

      await clock.tick(50)

      expect(callback).not.toHaveBeenCalled()
    })

    test("should not call timeout that has been cleared", async ({
      expect,
    }) => {
      const clock = new MockClockImplementation()
      const callback = vi.fn()

      const timeoutId = clock.setTimeout(callback, 100)
      clock.clearTimeout(timeoutId)

      await clock.tick(100)

      expect(callback).not.toHaveBeenCalled()
    })

    test("should call callbacks asynchronously", async ({ expect }) => {
      const clock = new MockClockImplementation()
      const callback = vi.fn()

      clock.setTimeout(callback, 100)

      const tickPromise = clock.tick(200)

      expect(callback).not.toHaveBeenCalled()

      await tickPromise

      expect(callback).toHaveBeenCalledTimes(1)
    })

    test("should show the correct time during a callback", async ({
      expect,
    }) => {
      const clock = new MockClockImplementation(1000)
      let observedTime: number | undefined

      const callback = vi.fn(() => {
        observedTime = clock.now()
      })

      clock.setTimeout(callback, 100)

      await clock.tick(200)

      expect(observedTime).toBe(1100)
      expect(clock.now()).toBe(1200)
    })

    test("should show the correct time during a callback for timeouts scheduled during a callback", async ({
      expect,
    }) => {
      const clock = new MockClockImplementation(1000)
      let observedTime: number | undefined

      const callback1 = vi.fn(() => {
        clock.setTimeout(callback2, 100)
      })
      const callback2 = vi.fn(() => {
        observedTime = clock.now()
      })

      clock.setTimeout(callback1, 100)

      await clock.tick(300)

      expect(observedTime).toBe(1200)
      expect(clock.now()).toBe(1300)
    })

    test("should throw when trying to tick while ticking", async ({
      expect,
    }) => {
      const clock = new MockClockImplementation()

      const tickPromise = clock.tick(100)

      await expect(() =>
        clock.tick(100),
      ).rejects.toThrowErrorMatchingInlineSnapshot(
        `[Error: Cannot tick while already ticking]`,
      )

      await tickPromise
    })

    test("should call a timeout even if set during another timeout", async ({
      expect,
    }) => {
      const clock = new MockClockImplementation()
      const callback1 = vi.fn(() => {
        clock.setTimeout(callback2, 100)
      })
      const callback2 = vi.fn()

      clock.setTimeout(callback1, 100)

      await clock.tick(200)

      expect(callback1).toHaveBeenCalledTimes(1)
      expect(callback2).toHaveBeenCalledTimes(1)
    })

    test("should call later timeouts after earlier ones", async ({
      expect,
    }) => {
      const clock = new MockClockImplementation(1000)

      const observedOrder: [callbackId: number, time: number][] = []

      const callback1 = vi.fn(() => {
        observedOrder.push([1, clock.now()])
      })
      const callback2 = vi.fn(() => {
        observedOrder.push([2, clock.now()])
        clock.setTimeout(callback4, 200)
      })
      const callback3 = vi.fn(() => {
        observedOrder.push([3, clock.now()])
      })
      const callback4 = vi.fn(() => {
        observedOrder.push([4, clock.now()])
      })
      const callback5 = vi.fn(() => {
        observedOrder.push([5, clock.now()])
      })

      clock.setTimeout(callback3, 300)
      clock.setTimeout(callback1, 100)
      clock.setTimeout(callback2, 200)
      clock.setTimeout(callback5, 500)

      await clock.tick(500)

      expect(callback1).toHaveBeenCalledTimes(1)
      expect(callback2).toHaveBeenCalledTimes(1)
      expect(observedOrder).toEqual([
        [1, 1100],
        [2, 1200],
        [3, 1300],
        [4, 1400],
        [5, 1500],
      ])
    })
  })

  describe("runUntilLast", () => {
    test("should run until the last timeout", async ({ expect }) => {
      const clock = new MockClockImplementation(1000)

      const callback1 = vi.fn()
      const callback2 = vi.fn()
      const callback3 = vi.fn()

      clock.setTimeout(callback1, 100)
      clock.setTimeout(callback2, 200)
      clock.setTimeout(callback3, 300)

      await clock.runUntilLast()

      expect(callback1).toHaveBeenCalledTimes(1)
      expect(callback2).toHaveBeenCalledTimes(1)
      expect(callback3).toHaveBeenCalledTimes(1)
      expect(clock.now()).toBe(1300)
    })

    test("should not run timers that were scheduled at a time after the time of the originally last timeout", async ({
      expect,
    }) => {
      const clock = new MockClockImplementation(1000)

      const callback1 = vi.fn()
      const callback2 = vi.fn(() => {
        clock.setTimeout(callback3, 100)
      })
      const callback3 = vi.fn()

      clock.setTimeout(callback1, 100)
      clock.setTimeout(callback2, 200)

      await clock.runUntilLast()

      expect(callback1).toHaveBeenCalledTimes(1)
      expect(callback2).toHaveBeenCalledTimes(1)
      expect(callback3).not.toHaveBeenCalled()
      expect(clock.now()).toBe(1200)
    })
  })
})
