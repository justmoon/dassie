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
import {
  FrameType,
  type StreamFrame,
  streamPacketSchema,
} from "../packets/schema"
import { createInitialStreamState } from "../stream/initialize"
import { Stream } from "../stream/stream"
import type { ConnectionState } from "./state"

export async function handleConnectionPacket(
  state: ConnectionState,
  packet: IlpPreparePacket,
) {
  const decryptedData = await state.pskEnvironment.decrypt(packet.data)

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

  const responseFrames: StreamFrame[] = []

  async function serializeAndEncryptStreamResponse(
    type: typeof IlpType.Fulfill | typeof IlpType.Reject,
  ) {
    const responseStreamPacket = streamPacketSchema.serializeOrThrow({
      packetType: type,
      sequence: streamPacket.sequence,
      amount: packet.amount,
      frames: responseFrames,
    })

    return await state.pskEnvironment.encrypt(responseStreamPacket)
  }

  async function tryFulfill() {
    if (!isFulfillable) return reject("Condition did not match")

    const encryptedResponse = await serializeAndEncryptStreamResponse(
      IlpType.Fulfill,
    )

    return {
      type: IlpType.Fulfill,
      data: {
        fulfillment,
        data: encryptedResponse,
      },
    }
  }

  async function reject(message: string) {
    const encryptedResponse = await serializeAndEncryptStreamResponse(
      IlpType.Reject,
    )

    return {
      type: IlpType.Reject,
      data: {
        code: IlpErrorCode.F99_APPLICATION_ERROR,
        message,
        triggeredBy: state.configuration.address,
        data: encryptedResponse,
      },
    }
  }

  let totalShares = 0n
  const streamReceiveList: StreamReceiveListEntry[] = []

  for (const frame of streamPacket.frames) {
    if (frame.type === FrameType.StreamMoney) {
      if (frame.data.shares === 0n) {
        return reject("StreamMoney frame with zero shares")
      }

      totalShares += frame.data.shares

      const streamId = Number(frame.data.streamId)

      let streamState = state.streams.get(streamId)
      if (!streamState) {
        streamState = createInitialStreamState()
        state.streams.set(streamId, streamState)

        state.topics.stream.emit(new Stream(streamState, streamId))
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
    return reject("Amount exceeds maximum receive amount")
  }

  for (const { streamId, amount } of allocatedAmounts) {
    const streamState = state.streams.get(streamId)
    if (!streamState) {
      throw new Error("Stream state not found")
    }

    streamState.receivedAmount += amount

    streamState.topics.money.emit(amount)
  }

  return tryFulfill()
}
