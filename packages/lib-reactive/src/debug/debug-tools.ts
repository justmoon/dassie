import { isObject } from "@dassie/lib-type-utils"

import type { Effect } from "../effect"
import type { ContextState, Reactor } from "../reactor"
import { isSignal } from "../signal"
import { TopicFactory, createTopic, isTopic } from "../topic"

export interface FirehoseEvent {
  topic: TopicFactory
  message: unknown
}

// eslint-disable-next-line unicorn/prevent-abbreviations
type ContextWeakRef = WeakRef<Record<keyof never, unknown>>

export const debugFirehose: TopicFactory<FirehoseEvent, FirehoseEvent> = () =>
  createTopic()

export class DebugTools {
  private context = new Set<ContextWeakRef>()
  private registry = new FinalizationRegistry<ContextWeakRef>(
    (value: ContextWeakRef) => {
      this.context.delete(value)
    }
  )

  constructor(
    readonly useContext: Reactor["use"],
    readonly contextState: ContextState
  ) {}

  notifyOfInstantiation<T>(factory: Effect<never, T>, value: T) {
    if (factory === (debugFirehose as Effect<never>)) {
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
        firehose.emit({ topic: factory as unknown as TopicFactory, message })
      })
    }
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

export const createDebugTools = import.meta.env.DEV
  ? (fromContext: Reactor["use"], topicsCache: ContextState) =>
      new DebugTools(fromContext, topicsCache)
  : () => undefined
