import {
  IlpErrorCode,
  type IlpPacketHandler,
  IlpType,
} from "@dassie/lib-protocol-ilp"

import { Connection } from "../connection/connection"
import { handleConnectionPacket } from "../connection/handle-packet"
import { createInitialConnectionState } from "../connection/initial-state"
import type { ServerState } from "./state"

export function createPacketHandler(
  state: Pick<
    ServerState,
    | "context"
    | "configuration"
    | "activeConnections"
    | "activeCredentials"
    | "topics"
  >,
) {
  return async function handleServerPacket(packet) {
    if (!packet.destination.startsWith(state.configuration.address)) {
      throw new Error("Received packet for wrong destination")
    }

    const localId = packet.destination.slice(
      state.configuration.address.length + 1,
    )

    const connection = state.activeConnections.get(localId)
    if (connection) {
      return handleConnectionPacket(connection, packet)
    }

    const secret = state.activeCredentials.get(localId)
    if (secret) {
      const connectionState = createInitialConnectionState({
        context: state.context,
        configuration: state.configuration,
        secret,
        side: "server",
      })
      state.activeConnections.set(localId, connectionState)

      state.topics.connection.emit(new Connection(connectionState))

      return handleConnectionPacket(connectionState, packet)
    }

    state.context.logger.warn(
      "received packet that did not match any connection or unused credential",
      { id: localId },
    )

    return {
      type: IlpType.Reject,
      data: {
        code: IlpErrorCode.F99_APPLICATION_ERROR,
        message: "Not implemented",
        triggeredBy: state.configuration.address,
        data: new Uint8Array(),
      },
    }
  } satisfies IlpPacketHandler
}
