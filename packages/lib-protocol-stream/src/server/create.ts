import type { IldcpResponse } from "@dassie/lib-protocol-ildcp"
import { createTopic } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import type { StreamProtocolContext } from "../context/context"
import { createPacketHandler } from "./handle-packet"
import { queryIldcp } from "./query-ildcp"
import { Server } from "./server"
import type { ServerState } from "./state"

interface ServerOptions {
  context: StreamProtocolContext
}

interface ServerStateOptions extends ServerOptions {
  configuration: IldcpResponse
}

export function createInitialServerState({
  context,
  configuration,
}: ServerStateOptions): ServerState {
  return {
    context,
    activeCredentials: new Map(),
    activeConnections: new Map(),
    configuration,
    topics: {
      connection: createTopic(),
    },
  }
}

export async function createServer(options: ServerOptions) {
  const configuration = await queryIldcp(options.context)

  if (isFailure(configuration)) {
    return configuration
  }

  const state = createInitialServerState({
    ...options,
    configuration,
  })

  const unregisterHandler = state.context.endpoint.handlePackets(
    createPacketHandler(state),
  )
  state.context.scope.onCleanup(unregisterHandler)

  return new Server(state)
}
