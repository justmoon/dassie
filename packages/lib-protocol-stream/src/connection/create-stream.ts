import { createInitialStreamState } from "../stream/initialize"
import type { ConnectionState } from "./state"

interface CreateStreamOptions {
  state: ConnectionState
}

export function createStream({ state }: CreateStreamOptions) {
  const streamId = state.nextStreamId
  state.nextStreamId += 2
  const streamState = createInitialStreamState()
  state.streams.set(streamId, streamState)
  return { streamState, streamId }
}
