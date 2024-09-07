import type { Listener, Topic } from "@dassie/lib-reactive"

import { createInitialStreamState } from "../stream/initialize"
import { Stream } from "../stream/stream"
import type { EventEmitter } from "../types/event-emitter"
import { measureExchangeRate } from "./measure-exchange-rate"
import type { ConnectionEvents, ConnectionState } from "./state"

export class Connection implements EventEmitter<ConnectionEvents> {
  constructor(private readonly state: ConnectionState) {}

  async measureExchangeRate() {
    return await measureExchangeRate({ state: this.state })
  }

  createStream() {
    const streamId = this.state.nextStreamId
    this.state.nextStreamId += 2
    const streamState = createInitialStreamState()
    this.state.streams.set(streamId, streamState)
    return new Stream(this.state, streamState, streamId)
  }

  on<TEventType extends keyof ConnectionEvents>(
    eventType: TEventType,
    handler: Listener<ConnectionEvents[TEventType]>,
  ) {
    const topic: Topic<ConnectionEvents[TEventType]> =
      this.state.topics[eventType]
    topic.on(undefined, handler)
  }

  off(eventType: keyof ConnectionEvents, handler: Listener<unknown>) {
    this.state.topics[eventType].off(handler)
  }
}
