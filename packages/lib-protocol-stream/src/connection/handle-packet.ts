import { areUint8ArraysEqual, uint8ArrayToHex } from "uint8array-extras"

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
  const logger = state.context.logger

  logger.debug?.("received packet", {
    packet,
  })

  const decryptedData = await state.pskEnvironment.decrypt(packet.data)

  if (isFailure(decryptedData)) {
    logger.warn("failed to decrypt STREAM packet data", {
      error: decryptedData,
    })
    return {
      type: IlpType.Reject,
      data: {
        code: IlpErrorCode.F06_UNEXPECTED_PAYMENT,
        message: "Failed to decrypt incoming STREAM packet data",
        triggeredBy: state.configuration.address,
        data: new Uint8Array(),
      },
    }
  }

  const streamPacketParseResult = streamPacketSchema.parse(decryptedData)

  if (isFailure(streamPacketParseResult)) {
    logger.warn("failed to parse STREAM packet data", {
      error: streamPacketParseResult,
    })
    return {
      type: IlpType.Reject,
      data: {
        code: IlpErrorCode.F06_UNEXPECTED_PAYMENT,
        message: "Failed to parse incoming STREAM packet data",
        triggeredBy: state.configuration.address,
        data: new Uint8Array(),
      },
    }
  }

  const streamPacket = streamPacketParseResult.value

  const fulfillment = await state.pskEnvironment.getFulfillment(packet.data)

  const condition = await state.context.crypto.hash("sha256", fulfillment)

  const isFulfillable = areUint8ArraysEqual(
    condition,
    packet.executionCondition,
  )

  if (!isFulfillable) {
    logger.debug?.("received unfulfillable STREAM packet", {
      expectedCondition: uint8ArrayToHex(packet.executionCondition),
      generatedCondition: uint8ArrayToHex(condition),
    })
  }

  const responseBuilder = createResponseBuilder({
    state,
    fulfillment: isFulfillable ? fulfillment : false,
    amount: packet.amount,
    sequence: streamPacket.sequence,
  })

  if (streamPacket.packetType !== IlpType.Prepare) {
    return responseBuilder.reject(
      "Wrong packet type",
      IlpErrorCode.F06_UNEXPECTED_PAYMENT,
    )
  }

  // Handle control frames
  for (const frame of streamPacket.frames) {
    switch (frame.type) {
      case FrameType.ConnectionNewAddress: {
        responseBuilder.setAssetDetails(state.configuration)
        state.remoteAddress = frame.data.sourceAccount

        break
      }
      case FrameType.ConnectionMaxStreamId: {
        state.remoteMaxStreamId = Number(frame.data.maxStreamId)

        break
      }
      case FrameType.ConnectionAssetDetails: {
        state.remoteAssetDetails = {
          assetCode: frame.data.sourceAssetCode,
          assetScale: frame.data.sourceAssetScale,
        }

        break
      }
      // No default
    }
  }

  if (streamPacket.amount > packet.amount) {
    return responseBuilder.reject(
      "Packet did not deliver enough money",
      IlpErrorCode.F04_INSUFFICIENT_DESTINATION_AMOUNT,
    )
  }

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

  state.context.logger.debug?.("allocated amounts", {
    amounts: allocatedAmounts,
  })

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
