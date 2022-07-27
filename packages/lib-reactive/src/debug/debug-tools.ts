import type { ContextState, Factory, Reactor } from "../reactor"
import { isStore } from "../store"
import { TopicFactory, createTopic, isTopic } from "../topic"

export interface FirehoseEvent {
  topic: TopicFactory
  message: unknown
}

export const debugFirehose = () => createTopic<FirehoseEvent>()

export class DebugTools {
  constructor(
    readonly useContext: Reactor["useContext"],
    readonly contextState: ContextState
  ) {}

  notifyOfInstantiation<T>(factory: Factory<T>, value: T) {
    if (factory === debugFirehose) {
      return
    }

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
    return [...this.contextState.values()].filter((possibleStore) =>
      isStore(possibleStore)
    )
  }
}

export const createDebugTools = import.meta.env.DEV
  ? (fromContext: Reactor["useContext"], topicsCache: ContextState) =>
      new DebugTools(fromContext, topicsCache)
  : () => undefined
