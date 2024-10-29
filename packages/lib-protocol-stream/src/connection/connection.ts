import type { Listener, Topic } from "@dassie/lib-reactive"

import type { Ratio } from "../math/ratio"
import { Stream } from "../stream/stream"
import type { EventEmitter } from "../types/event-emitter"
import { closeConnection } from "./close"
import { createStream } from "./create-stream"
import { makePayment } from "./make-payment"
import { measureExchangeRate } from "./measure-exchange-rate"
import { sendUntilDone } from "./send-until-done"
import {
  dangerouslyIgnoreExchangeRate,
  dangerouslyMeasureExchangeRate,
  setExchangeRate,
} from "./set-exchange-rate"
import type { ConnectionEvents, ConnectionState } from "./state"

interface PayOptions {
  sourceAmountLimit: bigint
}

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
    const { streamId, streamState } = createStream({ state: this.state })

    return new Stream(this.state, streamState, streamId)
  }

  pay({ sourceAmountLimit }: PayOptions) {
    return makePayment({ state: this.state, sourceAmountLimit })
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
