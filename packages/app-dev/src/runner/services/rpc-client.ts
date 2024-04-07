import { createConnection } from "node:net"

import { createActor } from "@dassie/lib-reactive"
import { createClient, createNodejsSocketLink } from "@dassie/lib-rpc/client"

import { DEBUG_RUNNER_RPC_PORT } from "../../backend/constants/ports"
import type { RunnerRpcRouter } from "../../backend/rpc-routers/runner-rpc-router"
import { transformer } from "../../common/utils/transformer"

export const RpcClientServiceActor = () =>
  createActor((sig) => {
    const socket = createConnection({
      port: DEBUG_RUNNER_RPC_PORT,
    })

    const rpcClient = createClient<RunnerRpcRouter>({
      connection: createNodejsSocketLink(socket),
      transformer,
    })

    sig.onCleanup(() => {
      rpcClient.close()
    })

    return rpcClient.rpc
  })
