import { remoteReactiveRouter } from "./remote-reactive-router"
import { runnerRpcRouter } from "./runner-rpc-router"
import { trpc } from "./trpc"
import { uiRpcRouter } from "./ui-rpc-router"

export const appRouter = trpc.mergeRouters(
  remoteReactiveRouter,
  trpc.router({
    ui: uiRpcRouter,
    runner: runnerRpcRouter,
  })
)

export type AppRouter = typeof appRouter
