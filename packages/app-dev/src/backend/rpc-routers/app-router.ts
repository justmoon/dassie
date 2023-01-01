import { remoteReactiveRouter } from "./remote-reactive-router"
import { runnerRpcRouter } from "./runner-rpc-router"
import { trpc } from "./trpc"
import { uiRpcRouter } from "./ui-rpc-router"

export const appRouter = trpc.mergeRouters(
  trpc.router({
    ui: uiRpcRouter,
    runner: runnerRpcRouter,
  }),
  remoteReactiveRouter
)

export type AppRouter = typeof appRouter
