import { type Listener, type Topic, createDeferred } from "@dassie/lib-reactive"

import { sendUntilDone } from "../connection/send-until-done"
import type { ConnectionState } from "../connection/state"
import type { EventEmitter } from "../types/event-emitter"
import { SEND_TIMEOUT_FAILURE, type SendFailure } from "./failures/send-failure"
import type { StreamEvents, StreamState } from "./state"

export interface SendOptions {
  amount: bigint
  timeout?: number
}

const DEFAULT_TIMEOUT = 30_000

export class Stream implements EventEmitter<StreamEvents> {
  constructor(
    private readonly connectionState: ConnectionState,
    private readonly state: StreamState,
    public readonly id: number,
  ) {}

  /**
   * Send a specified amount of money (in the sender's units) on this stream.
   *
   * The method will return a promise that either resolves when the money has
   * been sent successfully or return a failure if the money could not be sent
   * after the specified timeout.
   */
  send({
    amount,
    timeout = DEFAULT_TIMEOUT,
  }: SendOptions): Promise<void | SendFailure> {
    const deferred = createDeferred<void | SendFailure>()

    this.addSendAmount(amount)

    const targetAmount = this.state.sendMaximum

    const timeoutId = setTimeout(() => {
      this.state.topics.moneySent.off(listener)
      deferred.resolve(SEND_TIMEOUT_FAILURE)
    }, timeout)

    const listener = () => {
      if (this.state.sentAmount >= targetAmount) {
        this.state.topics.moneySent.off(listener)
        clearTimeout(timeoutId)
        deferred.resolve()
      }
    }
    this.state.topics.moneySent.on(undefined, listener)

    sendUntilDone(this.connectionState)

    return deferred
  }

  addSendAmount(amount: bigint) {
    this.state.sendMaximum += amount
  }

  addReceiveAmount(amount: bigint) {
    this.state.receiveMaximum += amount
  }

  on<TEventType extends keyof StreamEvents>(
    eventType: TEventType,
    handler: Listener<StreamEvents[TEventType]>,
  ) {
    const topic: Topic<StreamEvents[TEventType]> = this.state.topics[eventType]
    topic.on(undefined, handler)
  }

  off(eventType: keyof StreamEvents, handler: Listener<unknown>) {
    this.state.topics[eventType].off(handler)
  }
}
