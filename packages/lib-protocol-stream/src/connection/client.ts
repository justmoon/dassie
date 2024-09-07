import { isFailure } from "@dassie/lib-type-utils"

import type { StreamProtocolContext } from "../context/context"
import type { StreamCredentials } from "../server/generate-credentials"
import { queryIldcp } from "../server/query-ildcp"
import { Connection } from "./connection"
import { createInitialConnectionState } from "./initial-state"

interface ClientOptions {
  context: StreamProtocolContext
  credentials: StreamCredentials
}

export async function createClient({ context, credentials }: ClientOptions) {
  const configuration = await queryIldcp(context)

  if (isFailure(configuration)) {
    return configuration
  }

  const state = createInitialConnectionState({
    context,
    remoteAddress: credentials.destination,
    secret: credentials.secret,
    configuration,
    side: "client",
  })

  return new Connection(state)
}
