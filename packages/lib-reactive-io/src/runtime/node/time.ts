import {
  setImmediate as setImmediatePromise,
  setTimeout as setTimeoutPromise,
} from "node:timers/promises"

import {
  IntervalId,
  Time,
  TimeoutId,
  TimeoutOptions,
} from "@dassie/lib-reactive"

export class NodeTimeImplementation implements Time {
  now(): number {
    return Date.now()
  }

  setTimeout(callback: () => void, delay: number): TimeoutId {
    return setTimeout(callback, delay) as unknown as TimeoutId
  }

  clearTimeout(id: TimeoutId): void {
    clearTimeout(id as unknown as NodeJS.Timeout)
  }

  setInterval(callback: () => void, delay: number): IntervalId {
    return setInterval(callback, delay) as unknown as IntervalId
  }

  clearInterval(id: IntervalId): void {
    clearInterval(id as unknown as NodeJS.Timeout)
  }

  timeout(delay: number, options: TimeoutOptions = {}): Promise<void> {
    return setTimeoutPromise(delay, undefined, options)
  }

  immediate(): Promise<void> {
    return setImmediatePromise()
  }

  abortTimeout(delay: number): AbortSignal {
    return AbortSignal.timeout(delay)
  }
}
