import { createDeferred, createTopic } from "@dassie/lib-reactive"

import { createClient } from "../../client/client"
import type {
  AnyRouter,
  DeriveContextFromRouter,
} from "../../server/router/router"
import { createServer } from "../../server/server"

export function createServerClientPair<TRouter extends AnyRouter>(
  router: TRouter,
  context: DeriveContextFromRouter<TRouter>,
) {
  const toClient = createTopic<string>()
  const toServer = createTopic<string>()

  const closed = createDeferred()

  const server = createServer({ router })
  server.handleConnection({
    connection: {
      received: toServer,
      send: toClient.emit,
      closed,
    },
    context,
  })

  const client = createClient<typeof router>({
    connection: {
      received: toClient,
      send: toServer.emit,
      close: closed.resolve,
    },
  })

  return client
}
