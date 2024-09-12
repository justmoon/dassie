import type { Clock, TimeoutId } from "../types/base-modules/clock"

export interface MockClock extends Clock {
  tick(duration: number): Promise<void>
}

export const DEFAULT_MOCK_CLOCK_TIME = Number(new Date("2000-01-01"))

interface ScheduledTimeout {
  readonly id: TimeoutId
  readonly type: "timeout"
  readonly callback: () => void
  readonly time: number
}

export class MockClockImplementation implements MockClock {
  private unique = 0

  private timers: ScheduledTimeout[] = []
  private ticking = false

  constructor(private time: number = DEFAULT_MOCK_CLOCK_TIME) {}

  now(): number {
    return this.time
  }

  setTimeout(callback: () => void, delay: number): TimeoutId {
    const timeoutId = this.unique++ as unknown as TimeoutId
    this.insertTimer({
      id: timeoutId,
      type: "timeout",
      callback,
      time: this.time + delay,
    })
    return timeoutId
  }

  clearTimeout(id: TimeoutId): void {
    this.timers = this.timers.filter((timer) => timer.id !== id)
  }

  /**
   * Advances the clock by the specified amount of time and asynchronously executes
   * all timeouts and intervals scheduled before the new time.
   *
   * This method enters a new event loop tick before and after each callback.
   *
   * @param duration - The amount of time to advance the clock by in milliseconds.
   * @returns A promise that will resolve when all scheduled callbacks up to the
   *   specified time have been executed.
   */
  async tick(duration: number) {
    if (this.ticking) {
      throw new Error("Cannot tick while already ticking")
    }
    this.ticking = true

    const until = this.time + duration
    for (;;) {
      await this.macrotask()

      const nextTimer = this.timers[0]

      if (!nextTimer || nextTimer.time > until) {
        break
      }

      this.time = nextTimer.time

      this.timers.shift()

      nextTimer.callback()
    }

    this.time = until

    this.ticking = false
  }

  async runUntilLast() {
    const lastTime = this.timers.at(-1)?.time
    if (lastTime) {
      await this.tick(lastTime - this.time)
    }
  }

  private insertTimer(newTimer: ScheduledTimeout) {
    const index = this.timers.findIndex((timer) => newTimer.time < timer.time)

    if (index === -1) {
      this.timers.push(newTimer)
    } else {
      this.timers.splice(index, 0, newTimer)
    }
  }

  /**
   * Awaiting this method will run all scheduled micro-tasks before continuing.
   *
   * @returns A promise that will resolve on the next tick of the event loop.
   */
  private macrotask() {
    return new Promise<void>((resolve) => {
      setTimeout(() => {
        resolve()
      }, 0)
    })
  }
}

export function createMockClock(
  time: number = DEFAULT_MOCK_CLOCK_TIME,
): MockClock {
  return new MockClockImplementation(time)
}
