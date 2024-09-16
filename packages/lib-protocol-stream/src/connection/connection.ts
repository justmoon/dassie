import type { Listener, Topic } from "@dassie/lib-reactive"

import type { Ratio } from "../math/ratio"
import { createInitialStreamState } from "../stream/initialize"
import { Stream } from "../stream/stream"
import type { EventEmitter } from "../types/event-emitter"
import { closeConnection } from "./close"
import { measureExchangeRate } from "./measure-exchange-rate"
import { sendUntilDone } from "./send-until-done"
import {
  dangerouslyIgnoreExchangeRate,
  dangerouslyMeasureExchangeRate,
  setExchangeRate,
} from "./set-exchange-rate"
import type { ConnectionEvents, ConnectionState } from "./state"

export class Connection implements EventEmitter<ConnectionEvents> {
  constructor(private readonly state: ConnectionState) {}

  measureExchangeRate() {
    return measureExchangeRate({ state: this.state })
  }

  setExchangeRate(exchangeRate: Ratio) {
    return setExchangeRate(this.state, exchangeRate)
  }

  dangerouslyIgnoreExchangeRate() {
    dangerouslyIgnoreExchangeRate(this.state)
  }

  dangerouslyMeasureExchangeRate() {
    return dangerouslyMeasureExchangeRate(this.state)
  }

  createStream() {
    const streamId = this.state.nextStreamId
    this.state.nextStreamId += 2
    const streamState = createInitialStreamState()
    this.state.streams.set(streamId, streamState)
    return new Stream(this.state, streamState, streamId)
  }

  async close() {
    return closeConnection(this.state)
  }

  async closeAfterDone() {
    await sendUntilDone(this.state)
    return closeConnection(this.state)
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
