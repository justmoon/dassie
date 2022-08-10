import * as trpc from "@trpc/server"
import { z } from "zod"

import type { Reactor } from "@xen-ilp/lib-reactive"

import { logLineTopic } from "../features/logs"
import { peerTrafficTopic } from "../topics/peer-traffic"
import { createCliOnlyLogger } from "../utils/cli-only-logger"

export const runnerRpcRouter = trpc
  .router<Reactor>()
  .mutation("notifyPeerTraffic", {
    input: z.object({
      from: z.string(),
      to: z.string(),
    }),
    resolve({ input: peerMessageMetadata, ctx: reactor }) {
      reactor.useContext(peerTrafficTopic).emit(peerMessageMetadata)
    },
  })
  .mutation("notifyLogLine", {
    input: z.tuple([
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
    ]),
    resolve({ input: [nodeId, logLine], ctx: reactor }) {
      reactor.useContext(logLineTopic).emit({
        node: nodeId,
        ...logLine,
      })
      if (process.env["XEN_LOG_TO_CLI"] === "true") {
        const prefix = `◼ ${nodeId}`
        const cliLogger = createCliOnlyLogger(prefix)
        cliLogger.info(logLine.message, logLine.data)
      }
    },
  })

export type RunnerRpcRouter = typeof runnerRpcRouter
