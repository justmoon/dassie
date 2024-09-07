import { areUint8ArraysEqual } from "uint8array-extras"

import {
  IlpErrorCode,
  type IlpPreparePacket,
  IlpType,
} from "@dassie/lib-protocol-ilp"
import { isFailure } from "@dassie/lib-type-utils"

import {
  type StreamReceiveListEntry,
  allocateAmounts,
} from "../math/allocate-amounts"
import { FrameType, streamPacketSchema } from "../packets/schema"
import { createResponseBuilder } from "./create-response"
import { handleNewStream } from "./handle-new-stream"
import type { ConnectionState } from "./state"

export async function handleConnectionPacket(
  state: ConnectionState,
  packet: IlpPreparePacket,
) {
  const decryptedData = await state.pskEnvironment.decrypt(packet.data)

  if (isFailure(decryptedData)) {
    return {
      type: IlpType.Reject,
      data: {
        code: IlpErrorCode.F99_APPLICATION_ERROR,
        message: "Failed to decrypt packet",
        triggeredBy: state.configuration.address,
        data: new Uint8Array(),
      },
    }
  }

  const streamPacketParseResult = streamPacketSchema.parse(decryptedData)

  if (isFailure(streamPacketParseResult)) {
    return {
      type: IlpType.Reject,
      data: {
        code: IlpErrorCode.F99_APPLICATION_ERROR,
        message: "Failed to parse packet",
        triggeredBy: state.configuration.address,
        data: new Uint8Array(),
      },
    }
  }

  const streamPacket = streamPacketParseResult.value

  const fulfillment = await state.pskEnvironment.getFulfillment(packet.data)

  const condition = await state.context.crypto.hash(fulfillment)

  const isFulfillable = areUint8ArraysEqual(
    condition,
    packet.executionCondition,
  )

  const responseBuilder = createResponseBuilder({
    state,
    fulfillment: isFulfillable ? fulfillment : false,
    amount: packet.amount,
    sequence: streamPacket.sequence,
  })

  let totalShares = 0n
  const streamReceiveList: StreamReceiveListEntry[] = []

  for (const frame of streamPacket.frames) {
    if (frame.type === FrameType.StreamMoney) {
      // Skip zero-valued frames
      if (frame.data.shares === 0n) continue

      totalShares += frame.data.shares

      const streamId = Number(frame.data.streamId)

      const streamState =
        state.streams.get(streamId) ?? handleNewStream({ state, streamId })
      if (isFailure(streamState)) {
        responseBuilder.setStreamClose(
          streamId,
          streamState.errorCode,
          streamState.reason,
        )
        continue
      }

      streamReceiveList.push({
        streamId,
        shares: frame.data.shares,
        receiveMaximum: streamState.receiveMaximum - streamState.receivedAmount,
      })
    }
  }

  const allocatedAmounts = allocateAmounts(
    totalShares,
    packet.amount,
    streamReceiveList,
  )

  if (isFailure(allocatedAmounts)) {
    return responseBuilder.reject("Amount exceeds maximum receive amount")
  }

  for (const { streamId, amount } of allocatedAmounts) {
    const streamState = state.streams.get(streamId)
    if (!streamState) {
      throw new Error("Stream state not found")
    }

    streamState.receivedAmount += amount

    streamState.topics.money.emit(amount)
  }

  return responseBuilder.tryFulfill()
}
