import { z } from "zod"

import { LogsStore } from "../../common/stores/logs"
import { PeeringStateStore } from "../stores/peering-state"
import { PeerTrafficTopic } from "../topics/peer-traffic"
import { shortenNodeId } from "../utils/shorten-node-id"
import { trpc } from "./trpc"

export const runnerRpcRouter = trpc.router({
  notifyPeerTraffic: trpc.procedure
    .input(
      z.object({
        from: z.string(),
        to: z.string(),
      }),
    )
    .mutation(({ input: peerMessageMetadata, ctx: { reactor } }) => {
      reactor.use(PeerTrafficTopic).emit(peerMessageMetadata)
    }),
  notifyLogLine: trpc.procedure
    .input(
      z.object({
        type: z.union([
          z.literal("debug"),
          z.literal("info"),
          z.literal("warn"),
          z.literal("error"),
        ]),
        namespace: z.string(),
        node: z.string(),
        date: z.number(),
        message: z.string(),
        parameters: z.array(z.unknown()),
        caller: z.union([z.string(), z.undefined()]),
      }),
    )
    .mutation(({ input: logEvent, ctx: { reactor } }) => {
      const logs = reactor.use(LogsStore)

      logEvent.node = shortenNodeId(logEvent.node)

      logs.addLogLine({
        ...logEvent,
        caller: logEvent.caller ?? undefined,
      })
    }),
  notifyPeerState: trpc.procedure
    .input(
      z.object({
        nodeId: z.string(),
        peers: z.array(z.string()),
      }),
    )
    .mutation(({ input: { nodeId, peers }, ctx: { reactor } }) => {
      const peeringState = reactor.use(PeeringStateStore)
      peeringState.updateNodePeers(nodeId, peers)
    }),
})

export type RunnerRpcRouter = typeof runnerRpcRouter
