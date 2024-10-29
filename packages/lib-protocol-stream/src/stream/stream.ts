import { type Listener, type Topic } from "@dassie/lib-reactive"

import type { NoExchangeRateFailure } from "../connection/failures/no-exchange-rate-failure"
import type { NoRemoteAddressFailure } from "../connection/failures/no-remote-address-failure"
import type { ConnectionState } from "../connection/state"
import type { EventEmitter } from "../types/event-emitter"
import { addSendAmount } from "./add-send-amount"
import { closeStream } from "./close"
import { type SendFailure } from "./failures/send-failure"
import { sendAndAwait } from "./send-and-await"
import type { StreamEvents, StreamState } from "./state"

export interface SendOptions {
  amount: bigint
  timeout?: number
}

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
   * within the specified timeout.
   */
  send({
    amount,
    timeout,
  }: SendOptions): Promise<
    void | SendFailure | NoRemoteAddressFailure | NoExchangeRateFailure
  > {
    return sendAndAwait({
      connectionState: this.connectionState,
      state: this.state,
      amount,
      timeout,
    })
  }

  /**
   * Close the stream.
   *
   * This will immediately stop all further sending except for any packets that
   * are already in flight.
   */
  close() {
    closeStream(this.id, this.connectionState, this.state)
  }

  addSendAmount(amount: bigint) {
    return addSendAmount({
      connectionState: this.connectionState,
      state: this.state,
      amount,
    })
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
