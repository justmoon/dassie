import * as trpc from "@trpc/server"
import { z } from "zod"

import type { Reactor } from "@xen-ilp/lib-reactive"

import { globalFirehoseTopic } from "../topics/global-firehose"

export const startupTime = Date.now()

export const runnerRpcRouter = trpc
  .router<Reactor>()
  .mutation("notifyTopicMessage", {
    input: z.tuple([z.string(), z.string(), z.number()]),
    resolve({ input: [nodeId, topicName, messageId], ctx: reactor }) {
      reactor.useContext(globalFirehoseTopic).emit({
        nodeId,
        topic: topicName,
        messageId,
      })
    },
  })

export type RunnerRpcRouter = typeof runnerRpcRouter
