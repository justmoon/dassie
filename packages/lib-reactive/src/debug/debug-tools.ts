import { isObject } from "@dassie/lib-type-utils"

import type { ContextState, Factory, Reactor } from "../reactor"
import { isSignal } from "../signal"
import { type ReadonlyTopic, createTopic, isTopic } from "../topic"

export interface FirehoseEvent {
  topic: Factory<ReadonlyTopic>
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

  constructor(readonly reactor: Reactor, readonly contextState: ContextState) {}

  notifyOfContextInstantiation<T>(factory: Factory<T>, value: T) {
    if (factory === (debugFirehose as Factory<unknown>)) {
      return
    }

    if (!isObject(value)) {
      return
    }

    const weakReference = new WeakRef(value)
    this.context.add(weakReference)
    this.registry.register(value, weakReference)

    if (isTopic(value)) {
      const firehose = this.reactor.use(debugFirehose)
      value.on(this.reactor, (message) => {
        firehose.emit({
          topic: factory as unknown as Factory<ReadonlyTopic>,
          message,
        })
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
  ? (reactor: Reactor, topicsCache: ContextState) =>
      new DebugTools(reactor, topicsCache)
  : () => undefined
