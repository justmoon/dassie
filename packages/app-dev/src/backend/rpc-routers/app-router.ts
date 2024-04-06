import { createRouter } from "@dassie/lib-rpc/server"

import { runnerRpcRouter } from "./runner-rpc-router"
import { uiRpcRouter } from "./ui-rpc-router"

export const appRouter = createRouter({
  ui: uiRpcRouter,
  runner: runnerRpcRouter,
})

export type AppRouter = typeof appRouter
