import { createServer } from "node:net"

import { type Reactor, createActor } from "@dassie/lib-reactive"
import {
  createNodejsSocketAdapter,
  createServer as createRpcServer,
} from "@dassie/lib-rpc/server"

import { transformer } from "../../common/utils/transformer"
import { DEBUG_RUNNER_RPC_PORT } from "../constants/ports"
import { runnerRpcRouter } from "../rpc-routers/runner-rpc-router"

export const ServeRunnerRpcActor = (reactor: Reactor) =>
  createActor((sig) => {
    const socketServer = createServer()

    socketServer.listen(DEBUG_RUNNER_RPC_PORT)

    const rpcServer = createRpcServer({
      router: runnerRpcRouter,
      transformer,
    })

    socketServer.on("connection", (socket) => {
      rpcServer.handleConnection({
        connection: createNodejsSocketAdapter(socket),
        context: { sig, reactor },
      })
    })

    sig.onCleanup(() => {
      socketServer.close()
    })
  })
