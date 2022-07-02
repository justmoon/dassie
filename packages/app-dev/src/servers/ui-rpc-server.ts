import * as trpc from "@trpc/server"
import { applyWSSHandler } from "@trpc/server/adapters/ws"
import { WebSocketServer } from "ws"

import type { EventBroker } from "@xen-ilp/lib-events"

import type LogManager from "../classes/log-manager"
import { NodeLogLine, logLineTopic } from "../topics/log-message"

export interface UiRpcServerContext {
  logManager: LogManager
  eventBroker: EventBroker
}

export const startupTime = Date.now()

export const uiRpcRouter = trpc
  .router<UiRpcServerContext>()
  .query("startupTime", {
    resolve() {
      return startupTime
    },
  })
  .subscription("logs", {
    resolve({ ctx }) {
      return new trpc.Subscription<NodeLogLine>((emit) => {
        for (const logLine of ctx.logManager.store.get()) {
          emit.data(logLine)
        }
        const handleLogLine = (logLine: NodeLogLine) => {
          emit.data(logLine)
        }
        return ctx.eventBroker.addListener(logLineTopic, handleLogLine)
      })
    },
  })

export type UiRpcRouter = typeof uiRpcRouter

export default class UiRpcServer {
  readonly wss: WebSocketServer

  constructor(readonly context: UiRpcServerContext) {
    this.wss = new WebSocketServer({ port: 10_001 })
    applyWSSHandler<UiRpcRouter>({
      wss: this.wss,
      router: uiRpcRouter,
      createContext() {
        return context
      },
    })
  }
}
