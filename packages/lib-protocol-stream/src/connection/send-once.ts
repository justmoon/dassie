import { IlpType } from "@dassie/lib-protocol-ilp"
import { isFailure } from "@dassie/lib-type-utils"

import { FrameType, type StreamFrame } from "../packets/schema"
import {
  fulfillSend,
  getDesiredSendAmount,
  prepareSend,
  rejectSend,
} from "../stream/send-money"
import { sendPacket } from "./send-packet"
import type { ConnectionState } from "./state"

/**
 * Sends one Interledger packet and returns once the packet has been fulfilled or rejected.
 */
export async function sendOnce(state: ConnectionState) {
  const { context, streams, maximumPacketAmount, exchangeRate } = state

  if (exchangeRate === undefined) {
    throw new Error("Cannot send money without an exchange rate")
  }

  const frames = new Array<StreamFrame>()
  const fulfillHandlers = new Array<() => void>()
  const rejectHandlers = new Array<() => void>()

  let totalSend = 0n
  for (const [streamId, streamState] of streams.entries()) {
    const desiredSend = getDesiredSendAmount(streamState)

    const maximumSend = maximumPacketAmount - totalSend
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

  const minDestinationAmount = (totalSend * exchangeRate[0]) / exchangeRate[1]

  context.logger.debug?.("sending packet", {
    sourceAmount: totalSend,
    minDestinationAmount,
  })

  const result = await sendPacket({
    state,
    sourceAmount: totalSend,
    destinationAmount: minDestinationAmount,
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
}
