import * as trpc from "@trpc/server"
import { applyWSSHandler } from "@trpc/server/adapters/ws"
import { WebSocketServer } from "ws"

import type { EffectContext, Reactor } from "@xen-ilp/lib-reactive"

import { IndexedLogLine, indexedLogLineTopic } from "../features/logs"
import { logsStore } from "../stores/logs"

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

export type UiRpcRouter = typeof uiRpcRouter

export const listenForUiWebSocket = (sig: EffectContext) => {
  const wss = new WebSocketServer({ port: 10_001 })
  applyWSSHandler<UiRpcRouter>({
    wss,
    router: uiRpcRouter,
    createContext: () => sig.reactor,
  })
  sig.onCleanup(() => {
    wss.close()
  })
}
