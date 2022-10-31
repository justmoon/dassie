import { z } from "zod"

import { activeTemplateSignal } from "../signals/active-template"
import { trpc } from "./trpc"

export const uiRpcRouter = trpc.mergeRouters(
  trpc.router({
    addRandomNode: trpc.procedure
      // TRPC seems to throw an error when using superjson as a transformer on a method with no parameters.
      .input(z.object({}))
      .mutation(({ ctx: { reactor } }) => {
        const templateSignal = reactor.use(activeTemplateSignal)

        const template = templateSignal.read()

        const nodeCount = new Set(template.flat()).size

        const peers = Array.from({ length: Math.min(nodeCount, 3) })
          .fill(undefined)
          .map(() => Math.floor(Math.random() * nodeCount))

        const uniquePeers = [...new Set(peers)]

        templateSignal.update((nodes) => [...nodes, uniquePeers])
      }),
  })
)

export type UiRpcRouter = typeof uiRpcRouter
