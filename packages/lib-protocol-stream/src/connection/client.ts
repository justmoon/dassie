import { isFailure } from "@dassie/lib-type-utils"

import type { StreamProtocolContext } from "../context/context"
import { queryIldcp } from "../server/query-ildcp"
import { Connection } from "./connection"
import { createInitialConnectionState } from "./initial-state"

interface ClientOptions {
  context: StreamProtocolContext
  remoteAddress: string
  secret: Uint8Array
}

export async function createClient(options: ClientOptions) {
  const configuration = await queryIldcp(options.context)

  if (isFailure(configuration)) {
    return configuration
  }

  const state = createInitialConnectionState({
    ...options,
    configuration,
    side: "client",
  })

  return new Connection(state)
}
