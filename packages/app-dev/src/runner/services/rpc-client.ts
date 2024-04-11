import { createConnection } from "node:net"

import { type Reactor, createActor } from "@dassie/lib-reactive"
import {
  type Client,
  createClient,
  createNodejsSocketLink,
} from "@dassie/lib-rpc/client"

import { DEBUG_RUNNER_RPC_PORT } from "../../backend/constants/ports"
import type { RunnerRpcRouter } from "../../backend/rpc-routers/runner-rpc-router"
import { transformer } from "../../common/utils/transformer"

export type RpcReactor = Reactor<{
  rpc: Client<RunnerRpcRouter>["rpc"]
}>

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

    return rpcClient
  })
