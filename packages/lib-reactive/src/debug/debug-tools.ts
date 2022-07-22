import type { Factory } from "../factory"
import type { ContextState, Reactor } from "../reactor"
import { isStore } from "../store"
import { TopicFactory, createTopic, isTopic } from "../topic"

export interface FirehoseEvent {
  topic: TopicFactory
  message: unknown
}

export const debugFirehose = () => createTopic<FirehoseEvent>()

export class DebugTools {
  constructor(
    readonly fromContext: Reactor["fromContext"],
    readonly contextState: ContextState
  ) {}

  notifyOfInstantiation(factory: Factory<unknown>, value: unknown) {
    if (factory === debugFirehose) {
      return
    }

    if (isTopic(value)) {
      const firehose = this.fromContext(debugFirehose)
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
    return [...this.contextState.values()].filter((possibleStore) =>
      isStore(possibleStore)
    )
  }
}

export const createDebugTools = import.meta.env.DEV
  ? (fromContext: Reactor["fromContext"], topicsCache: ContextState) =>
      new DebugTools(fromContext, topicsCache)
  : () => undefined
