import { initTRPC } from "@trpc/server"
import superjson, { allowErrorProps } from "superjson"

import { ActorContext, Reactor } from "@dassie/lib-reactive"

export interface TrpcContext {
  sig: ActorContext
  reactor: Reactor
}

allowErrorProps("stack")

export const trpc = initTRPC.context<TrpcContext>().create({
  transformer: superjson,
})
