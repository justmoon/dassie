import type { IldcpResponse } from "@dassie/lib-protocol-ildcp"
import {
  type DisposableScope,
  confineScope,
  createScope,
  createTopic,
} from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import { closeConnection } from "../connection/close"
import type { StreamProtocolContext } from "../context/context"
import { createPacketHandler } from "./handle-packet"
import { queryIldcp } from "./query-ildcp"
import { Server } from "./server"
import type { ServerState } from "./state"

interface ServerOptions {
  context: StreamProtocolContext
}

interface ServerStateOptions extends ServerOptions {
  scope: DisposableScope
  configuration: IldcpResponse
}

export function createInitialServerState({
  context,
  scope,
  configuration,
}: ServerStateOptions): ServerState {
  return {
    context,
    scope,
    activeCredentials: new Map(),
    activeConnections: new Map(),
    configuration,
    topics: {
      connection: createTopic(),
    },
  }
}

export async function createServer(options: ServerOptions) {
  const scope = createScope("stream-server")
  confineScope(scope, options.context.scope)

  const configuration = await queryIldcp(options.context)

  if (isFailure(configuration)) {
    return configuration
  }

  const state = createInitialServerState({
    ...options,
    scope,
    configuration,
  })

  const unregisterHandler = state.context.endpoint.handlePackets(
    createPacketHandler(state),
  )
  scope.onCleanup(unregisterHandler)

  scope.onCleanup(async () => {
    for (const connection of state.activeConnections.values()) {
      const result = await closeConnection(connection)
      if (isFailure(result)) {
        // Ignore failures since we are closing the connection anyway
      }
    }
  })

  return new Server(state)
}
