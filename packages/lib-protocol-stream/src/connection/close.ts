import { IlpType } from "@dassie/lib-protocol-ilp"
import { isFailure } from "@dassie/lib-type-utils"

import { ErrorCode, FrameType } from "../packets/schema"
import { closeStream } from "../stream/close"
import { sendPacket } from "./send-packet"
import type { ConnectionState } from "./state"

export function markConnectionClosed(state: ConnectionState) {
  for (const [streamId, stream] of state.streams.entries()) {
    closeStream(streamId, state, stream)
  }

  if (!state.isClosed) {
    state.isClosed = true
    state.topics.closed.emit()
  }
}

export async function closeConnection(state: ConnectionState) {
  if (state.isClosed) return

  markConnectionClosed(state)
  return await sendClose(state)
}

export async function sendClose(state: ConnectionState) {
  const startTime = state.context.clock.now()
  for (let index = 0; index < state.context.policy.closeRetries; index++) {
    const currentTime = state.context.clock.now()
    if (currentTime - startTime > state.context.policy.closeTimeout) break

    const result = await sendPacket({
      state,
      sourceAmount: 0n,
      destinationAmount: 0n,
      fulfillable: true,
      frames: [
        {
          type: FrameType.ConnectionClose,
          data: {
            errorCode: ErrorCode.NoError,
            errorMessage: "",
          },
        },
      ],
    })

    if (isFailure(result)) {
      return result
    }

    if (result.ilp.type === IlpType.Fulfill) {
      return true
    }
  }

  return false
}
