import * as trpc from "@trpc/server"
import superjson from "superjson"

import type { Reactor } from "@xen-ilp/lib-reactive"

import { runnerRpcRouter } from "./runner-rpc-router"
import { uiRpcRouter } from "./ui-rpc-router"

export const appRouter = trpc
  .router<Reactor>()
  .transformer(superjson)
  .merge("ui.", uiRpcRouter)
  .merge("runner.", runnerRpcRouter)

export type AppRouter = typeof appRouter
