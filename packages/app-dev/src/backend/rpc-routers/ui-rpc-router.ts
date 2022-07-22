import * as trpc from "@trpc/server"

import type { Reactor } from "@xen-ilp/lib-reactive"

import { IndexedLogLine, indexedLogLineTopic } from "../features/logs"
import { logsStore } from "../stores/logs"
import {
  GlobalFirehoseMessage,
  globalFirehoseTopic,
} from "../topics/global-firehose"

export const startupTime = Date.now()

export const uiRpcRouter = trpc
  .router<Reactor>()
  .query("startupTime", {
    resolve() {
      return startupTime
    },
  })
  .subscription("logs", {
    resolve({ ctx: { fromContext } }) {
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
    resolve({ ctx: { fromContext } }) {
      return new trpc.Subscription<GlobalFirehoseMessage>((sendToClient) => {
        return fromContext(globalFirehoseTopic).on((logLine) => {
          sendToClient.data(logLine)
        })
      })
    },
  })

export type UiRpcRouter = typeof uiRpcRouter
