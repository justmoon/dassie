import { setImmediate, setTimeout } from "node:timers/promises"

import { Time, TimeoutOptions } from "@dassie/lib-reactive"

export class NodeTimeImplementation implements Time {
  now(): number {
    return Date.now()
  }

  timeout(delay: number, options: TimeoutOptions = {}): Promise<void> {
    return setTimeout(delay, undefined, options)
  }

  immediate(): Promise<void> {
    return setImmediate()
  }

  abortTimeout(delay: number): AbortSignal {
    return AbortSignal.timeout(delay)
  }
}
