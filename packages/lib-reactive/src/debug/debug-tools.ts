import { castImmutable } from "immer"

import { isObject } from "@dassie/lib-type-utils"

import type { ContextState, Reactor } from "../reactor"

export interface ContextEntry {
  uniqueId: number
  reference: WeakRef<Record<keyof never, unknown>>
}

class DebugTools {
  private static uniqueId = 0
  private context = new Map<number, ContextEntry>()
  private registry = new FinalizationRegistry<ContextEntry>(
    (value: ContextEntry) => {
      this.context.delete(value.uniqueId)
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

    const contextEntry = {
      uniqueId: DebugTools.uniqueId++,
      reference: new WeakRef(value),
    }
    this.context.set(contextEntry.uniqueId, contextEntry)
    this.registry.register(value, contextEntry)
  }

  /**
   * Retrieve the list of all current context entries
   *
   * @returns All currently instantiated values in the reactor.
   */
  getContext() {
    return castImmutable(this.context)
  }
}

export const createDebugTools = (
  reactor: Reactor,
  topicsCache: ContextState,
) => (import.meta.env.DEV ? new DebugTools(reactor, topicsCache) : undefined)
