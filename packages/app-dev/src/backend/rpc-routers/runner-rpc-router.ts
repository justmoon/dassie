import { z } from "zod"

import { logsStore } from "../../common/stores/logs"
import { peeringStateStore } from "../stores/peering-state"
import { peerTrafficTopic } from "../topics/peer-traffic"
import { trpc } from "./trpc"

export const runnerRpcRouter = trpc.router({
  notifyPeerTraffic: trpc.procedure
    .input(
      z.object({
        from: z.string(),
        to: z.string(),
      })
    )
    .mutation(({ input: peerMessageMetadata, ctx: { reactor } }) => {
      reactor.use(peerTrafficTopic).emit(peerMessageMetadata)
    }),
  notifyLogLine: trpc.procedure
    .input(
      z.tuple([
        z.string(),
        z.union([
          z.object({
            type: z.union([
              z.literal("debug"),
              z.literal("info"),
              z.literal("warn"),
              z.literal("error"),
            ]),
            namespace: z.string(),
            date: z.number(),
            message: z.string(),
            parameters: z.array(z.unknown()),
          }),
          z.object({
            type: z.literal("clear"),
            date: z.number(),
          }),
        ]),
      ])
    )
    .mutation(({ input: [nodeId, logEvent], ctx: { reactor } }) => {
      const logs = reactor.use(logsStore)
      if (logEvent.type === "clear") {
        logs.clear()
        return
      }

      logs.addLogLine({
        node: nodeId,
        ...logEvent,
      })
    }),
  notifyPeerState: trpc.procedure
    .input(
      z.object({
        nodeId: z.string(),
        peers: z.array(z.string()),
      })
    )
    .mutation(({ input: { nodeId, peers }, ctx: { reactor } }) => {
      const peeringState = reactor.use(peeringStateStore)
      peeringState.updateNodePeers(nodeId, peers)
    }),
})

export type RunnerRpcRouter = typeof runnerRpcRouter
