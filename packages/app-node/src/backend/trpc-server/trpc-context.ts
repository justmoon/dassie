import { initTRPC } from "@trpc/server"
import superjson from "superjson"

import type { ActorContext } from "@dassie/lib-reactive"

export const createContextFactory = (sig: ActorContext) => () => {
  return {
    sig,
  }
}

export type TrpcContext = ReturnType<ReturnType<typeof createContextFactory>>

export const trpc = initTRPC
  .context<TrpcContext>()
  .create({ transformer: superjson })
