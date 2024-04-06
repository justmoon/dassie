import { z } from "zod"

import { createRouter } from "@dassie/lib-rpc/server"

import { LogsStore } from "../../common/stores/logs"
import { PeeringStateStore } from "../stores/peering-state"
import { PeerTrafficTopic } from "../topics/peer-traffic"
import { shortenNodeId } from "../utils/shorten-node-id"
import { baseRoute } from "./route-types/base"

export const runnerRpcRouter = createRouter({
  notifyPeerTraffic: baseRoute
    .input(
      z.object({
        from: z.string(),
        to: z.string(),
      }),
    )
    .mutation(({ input: peerMessageMetadata, context: { reactor } }) => {
      reactor.use(PeerTrafficTopic).emit(peerMessageMetadata)
    }),
  notifyLogLine: baseRoute
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
    .mutation(({ input: logEvent, context: { reactor } }) => {
      const logs = reactor.use(LogsStore)

      logEvent.node = shortenNodeId(logEvent.node)

      logs.addLogLine({
        ...logEvent,
        caller: logEvent.caller ?? undefined,
      })
    }),
  notifyPeerState: baseRoute
    .input(
      z.object({
        nodeId: z.string(),
        peers: z.array(z.string()),
      }),
    )
    .mutation(({ input: { nodeId, peers }, context: { reactor } }) => {
      const peeringState = reactor.use(PeeringStateStore)
      peeringState.updateNodePeers(nodeId, peers)
    }),
})

export type RunnerRpcRouter = typeof runnerRpcRouter
