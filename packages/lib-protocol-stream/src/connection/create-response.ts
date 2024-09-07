import { IlpErrorCode, IlpType } from "@dassie/lib-protocol-ilp"

import {
  FrameType,
  type StreamFrame,
  streamPacketSchema,
} from "../packets/schema"
import type { ConnectionState } from "./state"

interface CreateResponseBuilderParameters {
  readonly state: ConnectionState
  readonly fulfillment: Uint8Array | false
  readonly amount: bigint
  readonly sequence: bigint
}

export function createResponseBuilder({
  state,
  fulfillment,
  amount,
  sequence,
}: CreateResponseBuilderParameters) {
  const closedStreams = new Map<
    number,
    { errorCode: number; errorMessage: string }
  >()

  async function serializeAndEncryptStreamResponse(
    type: typeof IlpType.Fulfill | typeof IlpType.Reject,
  ) {
    const responseFrames: StreamFrame[] = []

    for (const [streamId, { errorCode, errorMessage }] of closedStreams) {
      responseFrames.push({
        type: FrameType.StreamClose,
        data: {
          streamId: BigInt(streamId),
          errorCode,
          errorMessage,
        },
      })
    }

    const responseStreamPacket = streamPacketSchema.serializeOrThrow({
      packetType: type,
      sequence,
      amount,
      frames: responseFrames,
    })

    return await state.pskEnvironment.encrypt(responseStreamPacket)
  }

  async function tryFulfill() {
    state.context.logger.debug?.("trying to fulfill")
    if (!fulfillment) {
      return reject("Condition did not match")
    }

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
    state.context.logger.debug?.("rejecting", {
      message,
    })
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
  return {
    setStreamClose(streamId: number, errorCode: number, errorMessage: string) {
      // Capture the earliest reason for closing a stream
      if (closedStreams.has(streamId)) return

      closedStreams.set(streamId, { errorCode, errorMessage })
    },

    tryFulfill,
    reject,
  }
}
