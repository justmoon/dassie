import { z } from "zod"

import { logLineTopic } from "../features/logs"
import { peerTrafficTopic } from "../topics/peer-traffic"
import { createCliOnlyLogger } from "../utils/cli-only-logger"
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
      reactor.useContext(peerTrafficTopic).emit(peerMessageMetadata)
    }),
  notifyLogLine: trpc.procedure
    .input(
      z.tuple([
        z.string(),
        z.object({
          component: z.string(),
          date: z.string(),
          level: z.union([
            z.literal("debug"),
            z.literal("info"),
            z.literal("warn"),
            z.literal("error"),
            z.literal("clear"),
          ]),
          message: z.string(),
          data: z.record(z.string(), z.string()).optional(),
        }),
      ])
    )
    .mutation(({ input: [nodeId, logLine], ctx: { reactor } }) => {
      reactor.useContext(logLineTopic).emit({
        node: nodeId,
        ...logLine,
      })
      if (process.env["DASSIE_LOG_TO_CLI"] === "true") {
        const prefix = `◼ ${nodeId}`
        const cliLogger = createCliOnlyLogger(prefix)
        cliLogger.info(logLine.message, logLine.data)
      }
    }),
})

export type RunnerRpcRouter = typeof runnerRpcRouter
