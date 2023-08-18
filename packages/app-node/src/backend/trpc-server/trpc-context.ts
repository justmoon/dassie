import { initTRPC } from "@trpc/server"
import superjson from "superjson"

import type { ActorContext } from "@dassie/lib-reactive"

import { TrpcContext } from "./types/trpc-context"

export const createContextFactory = (sig: ActorContext) => (): TrpcContext => {
  return {
    sig,
    user: true as undefined | true,
  }
}

export const trpc = initTRPC
  .context<TrpcContext>()
  .create({ transformer: superjson })
