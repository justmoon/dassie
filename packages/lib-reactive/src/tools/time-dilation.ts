import type { Clock, TimeoutId } from "../types/base-modules/clock"

export interface TimeDilationClock extends Clock {
  /**
   * The current dilation factor.
   */
  readonly factor: number

  /**
   * Change the dilation factor.
   *
   * Any existing timers will still be in effect and will execute when the
   * dilated clock reaches their (virtual) expiration time.
   *
   * @param factor - The new factor by which the time should be dilated from now on.
   */
  setFactor(factor: number): void

  /**
   * Pause the clock.
   *
   * No timers will be executed while the clock is paused. The current time
   * shown by the clock will be frozen.
   */
  pause(): void

  /**
   * Resume the clock.
   *
   * Any existing timers will be reactivated.
   */
  resume(): void
}

interface TimeDilationClockPauseState {
  readonly type: "paused"
  readonly currentTime: number
}

interface TimeDilationClockRunningState {
  readonly type: "running"

  /**
   * The base clock time at which we started the time dilation.
   */
  readonly referenceTime: number

  /**
   * The dilated clock time at which we started the time dilation.
   *
   * When the clock is first started, this is the same as the reference time.
   * But when the clock is paused or the dilation factor is changed, the dilated
   * start time will diverge from the reference time.
   */
  readonly startTime: number
}

type TimeDilationClockState =
  | TimeDilationClockPauseState
  | TimeDilationClockRunningState

export class TimeDilationClockImplementation implements TimeDilationClock {
  private uniqueId = 0
  private state: TimeDilationClockState

  private readonly timers = new Map<
    TimeoutId,
    [upstreamId: TimeoutId | undefined, callback: () => void, time: number]
  >()

  constructor(
    private readonly base: Clock,
    public factor: number,
    initialTime?: number,
  ) {
    const now = this.base.now()
    this.state = {
      type: "running",
      referenceTime: now,
      startTime: initialTime ?? now,
    }
  }

  now() {
    return this.state.type === "paused" ?
        this.state.currentTime
      : this.state.startTime +
          (this.base.now() - this.state.referenceTime) * this.factor
  }

  setTimeout(callback: () => void, delay: number) {
    const id = this.uniqueId++ as unknown as TimeoutId
    const time = this.now() + delay

    const upstreamId =
      this.state.type === "paused" ?
        undefined
      : this.setUpstreamTimeout(id, callback, delay)
    this.timers.set(id, [upstreamId, callback, time])

    return id
  }

  clearTimeout(id: TimeoutId) {
    if (this.state.type === "running") {
      const upstreamId = this.timers.get(id)?.[0]
      if (upstreamId) this.base.clearTimeout(id)
    }
    this.timers.delete(id)
  }

  setFactor(newFactor: number) {
    this.factor = newFactor

    if (this.state.type === "running") {
      const now = this.now()

      this.state = {
        type: "running",
        referenceTime: this.base.now(),
        startTime: now,
      }

      // Update existing upstream timeouts to match the new factor
      for (const [id, [upstreamId, callback, time]] of this.timers) {
        const delay = Math.max(0, time - now)
        if (upstreamId) this.base.clearTimeout(upstreamId)
        const newUpstreamId = this.setUpstreamTimeout(id, callback, delay)

        this.timers.set(id, [newUpstreamId, callback, now + delay])
      }
    }
  }

  pause() {
    if (this.state.type === "paused") return

    this.state = {
      type: "paused",
      currentTime: this.now(),
    }

    // Clear all existing upstream timeouts
    for (const [, [upstreamId]] of this.timers) {
      if (upstreamId) this.base.clearTimeout(upstreamId)
    }
  }

  resume() {
    if (this.state.type === "running") return

    this.state = {
      type: "running",
      referenceTime: this.base.now(),
      startTime: this.state.currentTime,
    }

    // Create new upstream timeouts for all existing timers
    const now = this.now()
    for (const [id, [, callback, time]] of this.timers) {
      const delay = Math.max(0, time - now)
      const newUpstreamId = this.setUpstreamTimeout(id, callback, delay)
      this.timers.set(id, [newUpstreamId, callback, now + delay])
    }
  }

  private setUpstreamTimeout(
    id: TimeoutId,
    callback: () => void,
    delay: number,
  ) {
    return this.base.setTimeout(() => {
      this.timers.delete(id)
      callback()
    }, delay / this.factor)
  }
}

/**
 * Creates a new time dilation clock.
 *
 * Allows simulated time that is running faster or slower than the underlying
 * base clock.
 *
 * A factor of two means that for each second of real time, the clock will
 * advance by two seconds of simulated time. A factor of 0.5 means that for
 * each second of real time, the clock will advance by half a second of simulated
 * time.
 *
 * The dilated clock can also be paused and resumed and the dilation factor can
 * be changed.
 *
 * @param base - Reference clock to use as the basis for our time dilated clock.
 * @param factor - The factor by which the time is dilated.
 * @param initialTime - The time that the clock should show when it is first created.
 */
export function createTimeDilationClock(
  clock: Clock,
  factor: number,
  initialTime?: number,
): TimeDilationClock {
  return new TimeDilationClockImplementation(clock, factor, initialTime)
}
