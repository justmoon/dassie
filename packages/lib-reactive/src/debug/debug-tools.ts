import type { ContextState, Effect, Reactor } from "../reactor"
import { isStore } from "../store"
import { TopicFactory, createTopic, isTopic } from "../topic"

export interface FirehoseEvent {
  topic: TopicFactory
  message: unknown
}

export const debugFirehose = () => createTopic<FirehoseEvent>()

export class DebugTools {
  constructor(
    readonly fromContext: Reactor["use"],
    readonly contextState: ContextState
  ) {}

  notifyOfInstantiation(effect: Effect<never>) {
    if (effect === debugFirehose) {
      return
    }

    const value = this.contextState.get(effect)

    if (isTopic(value)) {
      const firehose = this.fromContext(debugFirehose)
      value.on((message) => {
        firehose.emit({ topic: effect as TopicFactory, message })
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
  ? (fromContext: Reactor["use"], topicsCache: ContextState) =>
      new DebugTools(fromContext, topicsCache)
  : () => undefined
