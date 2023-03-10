import { z } from "zod"

import { activeNodesStore } from "../stores/active-nodes"
import {
  VALID_PEERING_MODES,
  environmentSettingsStore,
} from "../stores/environment-settings"
import { generateNodeConfig } from "../utils/generate-node-config"
import { trpc } from "./trpc"

export const uiRpcRouter = trpc.mergeRouters(
  trpc.router({
    addRandomNode: trpc.procedure
      // TRPC seems to throw an error when using superjson as a transformer on a method with no parameters.
      .input(z.object({}))
      .mutation(({ ctx: { reactor } }) => {
        const activeNodes = reactor.use(activeNodesStore)
        const environmentSettings = reactor.use(environmentSettingsStore).read()

        const nodeCount = activeNodes.read().length

        const peers = Array.from({ length: Math.min(nodeCount, 3) })
          .fill(undefined)
          .map(() => Math.floor(Math.random() * nodeCount))

        const uniquePeers = [...new Set(peers)]

        activeNodes.addNode(
          generateNodeConfig(nodeCount, uniquePeers, environmentSettings)
        )
      }),
    setPeeringMode: trpc.procedure
      .input(z.enum(VALID_PEERING_MODES))
      .mutation(({ ctx: { reactor }, input }) => {
        reactor.use(environmentSettingsStore).setPeeringMode(input)
      }),
  })
)

export type UiRpcRouter = typeof uiRpcRouter
