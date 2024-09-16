import { type Listener, type Topic, createDeferred } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import { assertConnectionCanSendMoney } from "../connection/assert-can-send"
import type { NoExchangeRateFailure } from "../connection/failures/no-exchange-rate-failure"
import type { NoRemoteAddressFailure } from "../connection/failures/no-remote-address-failure"
import { sendUntilDone } from "../connection/send-until-done"
import type { ConnectionState } from "../connection/state"
import type { EventEmitter } from "../types/event-emitter"
import { closeStream } from "./close"
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
   * within the specified timeout.
   */
  send({
    amount,
    timeout = DEFAULT_TIMEOUT,
  }: SendOptions): Promise<
    void | SendFailure | NoRemoteAddressFailure | NoExchangeRateFailure
  > {
    {
      const result = this.addSendAmount(amount)
      if (isFailure(result)) return Promise.resolve(result)
    }

    const deferred = createDeferred<void | SendFailure>()
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

    sendUntilDone(this.connectionState).catch((error: unknown) => {
      this.connectionState.context.logger.error(
        "unexpected error returned by send loop",
        {
          error,
        },
      )
    })

    return deferred
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
    {
      const result = assertConnectionCanSendMoney(this.connectionState)
      if (isFailure(result)) return result
    }

    this.state.sendMaximum += amount

    return
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
