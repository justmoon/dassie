import superjson from "superjson"

import { createActor } from "@dassie/lib-reactive"
import { type InboundConnection, createServer } from "@dassie/lib-rpc/server"

import type { DassieActorContext } from "../../base/types/dassie-base"
import { ipc as logger } from "../../logger/instances"
import { appRouter } from "../../rpc-server/app-router"

export const ServeLocalIpcActor = () =>
  createActor((sig: DassieActorContext) => {
    const rpcServer = createServer({
      router: appRouter,
      transformer: superjson,
    })

    return {
      handleConnection: (connection: InboundConnection) => {
        logger.debug?.("received ipc rpc server connection")

        rpcServer.handleConnection({
          connection,
          context: {
            sig,
            isAuthenticated: true,
          },
        })
      },
    }
  })
