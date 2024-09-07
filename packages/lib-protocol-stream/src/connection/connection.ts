import { IlpType } from "@dassie/lib-protocol-ilp"
import { isFailure } from "@dassie/lib-type-utils"
import type { Listener, Topic } from "@dassie/lib-reactive"

import { FrameType, type StreamFrame } from "../packets/schema"
import { createInitialStreamState } from "../stream/initialize"
import {
  fulfillSend,
  getDesiredSendAmount,
  prepareSend,
  rejectSend,
} from "../stream/send-money"
import { Stream } from "../stream/stream"
import type { EventEmitter } from "../types/event-emitter"
import { measureExchangeRate } from "./measure-exchange-rate"
import { sendPacket } from "./send-packet"
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
    return new Stream(streamState, streamId)
  }

  async flush() {
    const frames = new Array<StreamFrame>()
    const fulfillHandlers = new Array<() => void>()
    const rejectHandlers = new Array<() => void>()

    let totalSend = 0n
    for (const [streamId, streamState] of this.state.streams.entries()) {
      const desiredSend = getDesiredSendAmount(streamState)

      const maximumSend = this.state.maximumPacketAmount - totalSend
      const actualSend = desiredSend > maximumSend ? maximumSend : desiredSend

      if (actualSend > 0n) {
        prepareSend(streamState, actualSend)

        totalSend += actualSend
        frames.push({
          type: FrameType.StreamMoney,
          data: {
            streamId: BigInt(streamId),
            shares: actualSend,
          },
        })

        fulfillHandlers.push(() => {
          fulfillSend(streamState, actualSend)
        })
        rejectHandlers.push(() => {
          rejectSend(streamState, actualSend)
        })
      }
    }

    const result = await sendPacket({
      state: this.state,
      amount: totalSend,
      fulfillable: true,
      frames,
    })

    if (isFailure(result)) {
      for (const handler of rejectHandlers) handler()
      return
    }

    if (result.ilp.type === IlpType.Fulfill) {
      for (const handler of fulfillHandlers) handler()
    } else {
      for (const handler of rejectHandlers) handler()
      // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition
      if (result.ilp.type !== IlpType.Reject) {
        throw new Error("Unexpected ILP packet type")
      }
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
