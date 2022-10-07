import { runnerRpcRouter } from "./runner-rpc-router"
import { trpc } from "./trpc"
import { uiRpcRouter } from "./ui-rpc-router"

export const appRouter = trpc.mergeRouters(
  uiRpcRouter,
  trpc.router({
    runner: runnerRpcRouter,
  })
)

export type AppRouter = typeof appRouter
