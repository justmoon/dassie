import { isObject } from "@dassie/lib-type-utils"

import type { ContextState, Factory, Reactor } from "../reactor"
import { isStore } from "../store"
import { TopicFactory, createTopic, isTopic } from "../topic"

export interface FirehoseEvent {
  topic: TopicFactory
  message: unknown
}

// eslint-disable-next-line unicorn/prevent-abbreviations
type ContextWeakRef = WeakRef<Record<keyof never, unknown>>

export const debugFirehose = () => createTopic<FirehoseEvent>()

export class DebugTools {
  private context = new Set<ContextWeakRef>()
  private registry = new FinalizationRegistry<ContextWeakRef>(
    (value: ContextWeakRef) => {
      this.context.delete(value)
    }
  )

  constructor(
    readonly useContext: Reactor["useContext"],
    readonly contextState: ContextState
  ) {}

  notifyOfInstantiation<T>(factory: Factory<T>, value: T) {
    if (factory === debugFirehose) {
      return
    }

    if (!isObject(value)) {
      return
    }

    const weakReference = new WeakRef(value)
    this.context.add(weakReference)
    this.registry.register(value, weakReference)

    if (isTopic(value)) {
      const firehose = this.useContext(debugFirehose)
      value.on((message) => {
        firehose.emit({ topic: factory as TopicFactory, message })
      })
    }
  }

  /**
   * Retrieve the state of the reactor.
   *
   * @returns All currently instantiated stores in the reactor.
   */
  getState() {
    return [...this.context]
      .map((weakReference) => weakReference.deref())
      .filter((possibleStore) => isStore(possibleStore))
  }
}

export const createDebugTools = import.meta.env.DEV
  ? (fromContext: Reactor["useContext"], topicsCache: ContextState) =>
      new DebugTools(fromContext, topicsCache)
  : () => undefined
