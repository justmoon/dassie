import type { Clock, IntervalId, TimeoutId } from "@dassie/lib-reactive"

export class NodeClockImplementation implements Clock {
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
}
