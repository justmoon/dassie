import * as trpc from "@trpc/server"
import { applyWSSHandler } from "@trpc/server/adapters/ws"
import { WebSocketServer } from "ws"

import type { EffectContext, Reactor } from "@xen-ilp/lib-reactive"

import { logsStore } from "../stores/logs"
import { NodeLogLine, logLineTopic } from "../topics/log-message"

export const startupTime = Date.now()

export const uiRpcRouter = trpc
  .router<Reactor>()
  .query("startupTime", {
    resolve() {
      return startupTime
    },
  })
  .subscription("logs", {
    resolve({ ctx: { read, on } }) {
      return new trpc.Subscription<NodeLogLine>((sendToClient) => {
        const handleLogLine = (logLine: NodeLogLine) => {
          sendToClient.data(logLine)
        }

        for (const logLine of read(logsStore)) {
          sendToClient.data(logLine)
        }

        return on(logLineTopic, handleLogLine)
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
