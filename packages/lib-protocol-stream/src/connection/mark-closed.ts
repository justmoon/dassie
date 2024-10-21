import { closeStream } from "../stream/close"
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
