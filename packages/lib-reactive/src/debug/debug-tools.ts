import { isObject } from "@dassie/lib-type-utils"

import type { ContextState, Reactor } from "../reactor"
import { isSignal } from "../signal"

// eslint-disable-next-line unicorn/prevent-abbreviations
type ContextWeakRef = WeakRef<Record<keyof never, unknown>>

class DebugTools {
  private context = new Set<ContextWeakRef>()
  private registry = new FinalizationRegistry<ContextWeakRef>(
    (value: ContextWeakRef) => {
      this.context.delete(value)
    },
  )

  constructor(
    readonly reactor: Reactor,
    readonly contextState: ContextState,
  ) {}

  notifyOfContextInstantiation<T>(value: T) {
    if (!isObject(value)) {
      return
    }

    const weakReference = new WeakRef(value)
    this.context.add(weakReference)
    this.registry.register(value, weakReference)
  }

  /**
   * Retrieve the state of the reactor.
   *
   * @returns All currently instantiated signals in the reactor.
   */
  getState() {
    return [...this.context]
      .map((weakReference) => weakReference.deref())
      .filter((possibleSignal) => isSignal(possibleSignal))
  }
}

export const createDebugTools = (
  reactor: Reactor,
  topicsCache: ContextState,
) => (import.meta.env.DEV ? new DebugTools(reactor, topicsCache) : undefined)
