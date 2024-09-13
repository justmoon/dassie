import { IlpType } from "@dassie/lib-protocol-ilp"
import { bigIntMin, isFailure } from "@dassie/lib-type-utils"

import {
  fulfillSend,
  getDesiredSendAmount,
  prepareSend,
  rejectSend,
} from "../stream/send-money"
import { createPrepareBuilder } from "./create-prepare"
import { sendPacket } from "./send-packet"
import type { ConnectionState } from "./state"

/**
 * Sends one Interledger packet and returns once the packet has been fulfilled or rejected.
 */
export async function sendOnce(state: ConnectionState) {
  const {
    context,
    configuration,
    streams,
    maxPacketAmount,
    exchangeRate,
    ourAddress,
    remoteKnowsAddress,
  } = state

  if (exchangeRate === undefined) {
    throw new Error("Cannot send money without an exchange rate")
  }

  const prepareBuilder = createPrepareBuilder({
    fulfillable: true,
    maxPacketAmount,
  })

  if (!remoteKnowsAddress) {
    prepareBuilder.setNewAddress(ourAddress)
    prepareBuilder.setAssetDetails(configuration)
    prepareBuilder.addStreamResponseHandler(() => {
      state.remoteKnowsAddress = true
    })
  }

  for (const [streamId, streamState] of streams.entries()) {
    const desiredSend = getDesiredSendAmount(streamState)
    const maxSend = prepareBuilder.getAvailableAmount()

    const actualSend = bigIntMin(maxSend, desiredSend)

    if (actualSend > 0n) {
      prepareSend(streamState, actualSend)

      prepareBuilder.addMoney({
        streamId,
        amount: actualSend,
        onFulfill: () => {
          fulfillSend(streamState, actualSend)
        },
        onReject: () => {
          rejectSend(streamState, actualSend)
        },
      })
    }
  }

  const packet = prepareBuilder.getPacket({ exchangeRate })

  context.logger.debug?.("sending packet", packet)

  const result = await sendPacket({
    state,
    ...packet,
  })

  if (isFailure(result)) {
    prepareBuilder.callRejectHandlers()
    return
  }

  if (result.ilp.type === IlpType.Fulfill) {
    prepareBuilder.callFulfillHandlers()
  } else {
    prepareBuilder.callRejectHandlers()
    // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition
    if (result.ilp.type !== IlpType.Reject) {
      throw new Error("Unexpected ILP packet type")
    }
  }

  if (!isFailure(result.stream)) {
    prepareBuilder.callStreamResponseHandlers()
  }
}
