import type { ConnectionState } from "../connection/state"
import type { StreamState } from "./state"

export function closeStream(
  streamId: number,
  connectionState: ConnectionState,
  stream: StreamState,
) {
  if (stream.isClosed) return

  connectionState.streams.delete(streamId)
  connectionState.closedStreams.set(streamId, stream)

  stream.isClosed = true
  stream.topics.closed.emit()
}
