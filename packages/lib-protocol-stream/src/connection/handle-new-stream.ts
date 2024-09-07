import { assert } from "@dassie/lib-logger"

import { createInitialStreamState } from "../stream/initialize"
import { Stream } from "../stream/stream"
import {
  CLIENT_STREAM_IDS_MUST_BE_ODD_FAILURE,
  MAXIMUM_STREAM_ID_EXCEEDED_FAILURE,
  SERVER_STREAM_IDS_MUST_BE_EVEN_FAILURE,
  STREAM_ID_ZERO_FAILURE,
} from "./failures/invalid-new-stream-failure"
import type { ConnectionState } from "./state"

interface HandleNewStreamParameters {
  readonly state: ConnectionState
  readonly streamId: number
}

export function handleNewStream({
  state,
  streamId,
}: HandleNewStreamParameters) {
  const {
    context: { logger },
    side,
    streams,
    topics,
    maximumStreamId,
  } = state

  assert(logger, !streams.has(streamId), "stream already exists")

  if (side === "server" && streamId % 2 === 0) {
    logger.warn("invalid stream ID - client stream IDs must be odd", {
      streamId,
    })
    return CLIENT_STREAM_IDS_MUST_BE_ODD_FAILURE
  }

  if (side === "client" && streamId % 2 === 1) {
    logger.warn("invalid stream ID - server stream IDs must be even", {
      streamId,
    })
    return SERVER_STREAM_IDS_MUST_BE_EVEN_FAILURE
  }

  if (streamId === 0) {
    logger.warn("invalid stream ID - stream ID must not be zero")
    return STREAM_ID_ZERO_FAILURE
  }

  if (streamId > maximumStreamId) {
    logger.warn("stream ID is above maximum", {
      streamId,
      maximumStreamId,
    })

    return MAXIMUM_STREAM_ID_EXCEEDED_FAILURE
  }

  const streamState = createInitialStreamState()
  streams.set(streamId, streamState)

  topics.stream.emit(new Stream(state, streamState, streamId))

  return streamState
}
