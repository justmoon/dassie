import { confineScope, createScope } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import type { StreamProtocolContext } from "../context/context"
import type { StreamCredentials } from "../server/generate-credentials"
import { queryIldcp } from "../server/query-ildcp"
import { closeConnection } from "./close"
import { Connection } from "./connection"
import { handleConnectionPacket } from "./handle-packet"
import { createInitialConnectionState } from "./initial-state"

interface ClientOptions {
  context: StreamProtocolContext
  credentials: StreamCredentials
}

export async function createClient({ context, credentials }: ClientOptions) {
  const scope = createScope(`stream-client-${credentials.destination}`)
  confineScope(scope, context.scope)

  const configuration = await queryIldcp(context)

  if (isFailure(configuration)) {
    return configuration
  }

  const state = createInitialConnectionState({
    context,
    scope,
    ourAddress: configuration.address,
    remoteAddress: credentials.destination,
    secret: credentials.secret,
    configuration,
    side: "client",
  })

  const unregisterHandler = state.context.endpoint.handlePackets((packet) =>
    handleConnectionPacket(state, packet),
  )
  scope.onCleanup(unregisterHandler)

  scope.onCleanup(async () => {
    const result = await closeConnection(state)
    if (isFailure(result)) {
      // Ignore failures since we are closing the connection anyway
    }
  })

  return new Connection(state)
}
