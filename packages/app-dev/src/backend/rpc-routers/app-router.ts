import type { Reactor } from "@dassie/lib-reactive"
import * as trpc from "@trpc/server"
import superjson from "superjson"

import { runnerRpcRouter } from "./runner-rpc-router"
import { uiRpcRouter } from "./ui-rpc-router"

export const appRouter = trpc
  .router<Reactor>()
  .transformer(superjson)
  .merge(uiRpcRouter)
  .merge("runner.", runnerRpcRouter)

export type AppRouter = typeof appRouter
