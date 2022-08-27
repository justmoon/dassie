import { applyWSSHandler } from "@trpc/server/adapters/ws"
import { WebSocketServer } from "ws"

import type { EffectContext } from "@dassie/lib-reactive"

import { AppRouter, appRouter } from "../rpc-routers/app-router"

export const listenForRpcWebSocket = (sig: EffectContext) => {
  const wss = new WebSocketServer({ port: 10_001 })
  const { broadcastReconnectNotification } = applyWSSHandler<AppRouter>({
    wss,
    router: appRouter,
    createContext: () => sig.reactor,
  })
  sig.onCleanup(() => {
    broadcastReconnectNotification()
    wss.close()
  })
}
