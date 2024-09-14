import { FrameType, type StreamFrame } from "../packets/schema"
import type { ResponseBuilder } from "./create-response"
import type { ConnectionState } from "./state"

interface HandleControlFrameParameters {
  state: ConnectionState
  frame: StreamFrame
  responseBuilder?: ResponseBuilder | undefined
}
export function handleControlFrame({
  state,
  frame,
  responseBuilder,
}: HandleControlFrameParameters) {
  switch (frame.type) {
    case FrameType.ConnectionNewAddress: {
      // Reset concurrency when changing addresses
      //
      // This helps prevent someone from creating a high-bandwidth connection
      // and then directing that traffic at a node that they don't control.
      //
      // By resetting concurrency, we will start sending slowly and only
      // ramp up if the other side responds to our packets using validly
      // encrypted and authenticated STREAM packets.
      if (state.remoteAddress !== frame.data.sourceAccount) {
        state.concurrency = state.context.policy.minimumConcurrency
      }

      responseBuilder?.setAssetDetails(state.configuration)
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
