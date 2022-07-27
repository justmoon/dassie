import { createTRPCClient } from "@trpc/client"
import * as trpc from "@trpc/server"
import superjson from "superjson"
import { z } from "zod"

import type { Reactor } from "@xen-ilp/lib-reactive"

import type { DebugRpcRouter } from "../../runner/effects/debug-rpc-server"
import { IndexedLogLine, indexedLogLineTopic } from "../features/logs"
import { activeTemplate } from "../stores/active-template"
import { logsStore } from "../stores/logs"
import {
  GlobalFirehoseMessage,
  globalFirehoseTopic,
} from "../topics/global-firehose"

export const startupTime = Date.now()

export const uiRpcRouter = trpc
  .router<Reactor>()
  .transformer(superjson)
  .query("startupTime", {
    resolve() {
      return startupTime
    },
  })
  .query("activeTemplate", {
    resolve({ ctx: reactor }) {
      return reactor.useContext(activeTemplate).read()
    },
  })
  .query("getPeerTable", {
    input: z.tuple([z.string()]),
    async resolve({ input: [nodeId], ctx: reactor }) {
      const node = reactor
        .useContext(activeTemplate)
        .read()
        .find(({ id }) => id === nodeId)

      if (!node) {
        throw new Error(`Unknown node ID ${nodeId}`)
      }

      const client = createTRPCClient<DebugRpcRouter>({
        url: `http://localhost:${node.debugPort}/trpc`,
        transformer: superjson,
      })

      return client.query("getPeerTable")
    },
  })
  .query("getMessage", {
    input: z.tuple([z.string(), z.number()]),
    async resolve({ input: [nodeId, messageId], ctx: reactor }) {
      const node = reactor
        .useContext(activeTemplate)
        .read()
        .find(({ id }) => id === nodeId)

      if (!node) {
        throw new Error(`Unknown node ID ${nodeId}`)
      }

      const client = createTRPCClient<DebugRpcRouter>({
        url: `http://localhost:${node.debugPort}/trpc`,
        transformer: superjson,
      })

      return client.query("getMessage", [messageId])
    },
  })
  .subscription("logs", {
    resolve({ ctx: { useContext: fromContext } }) {
      return new trpc.Subscription<IndexedLogLine>((sendToClient) => {
        const previousLogs = fromContext(logsStore).read()

        for (const logLine of previousLogs) {
          sendToClient.data(logLine)
        }

        return fromContext(indexedLogLineTopic).on((logLine) => {
          sendToClient.data(logLine)
        })
      })
    },
  })
  .subscription("globalFirehose", {
    resolve({ ctx: { useContext: fromContext } }) {
      return new trpc.Subscription<GlobalFirehoseMessage>((sendToClient) => {
        return fromContext(globalFirehoseTopic).on((logLine) => {
          sendToClient.data(logLine)
        })
      })
    },
  })

export type UiRpcRouter = typeof uiRpcRouter
