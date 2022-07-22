import * as trpc from "@trpc/server"
import { z } from "zod"

import type { Reactor } from "@xen-ilp/lib-reactive"

import { viteNodeServerFactory } from "../services/vite-node-server"
import { globalFirehoseTopic } from "../topics/global-firehose"

export const startupTime = Date.now()

export const runnerRpcRouter = trpc
  .router<Reactor>()
  .query("resolveId", {
    input: z.tuple([z.string(), z.string().nullish()]),
    async resolve({ input: [id, importer], ctx: reactor }) {
      const nodeServer = await reactor.fromContext(viteNodeServerFactory)
      return nodeServer.resolveId(id, importer ?? undefined)
    },
  })
  .query("fetchModule", {
    input: z.tuple([z.string()]),
    async resolve({ input, ctx: reactor }) {
      const nodeServer = await reactor.fromContext(viteNodeServerFactory)
      return nodeServer.fetchModule(...input)
    },
  })
  .mutation("notifyTopicMessage", {
    input: z.tuple([z.string(), z.string(), z.number()]),
    resolve({ input: [nodeId, topicName, messageId], ctx: reactor }) {
      reactor.fromContext(globalFirehoseTopic).emit({
        nodeId,
        topic: topicName,
        messageId,
      })
    },
  })

export type RunnerRpcRouter = typeof runnerRpcRouter
