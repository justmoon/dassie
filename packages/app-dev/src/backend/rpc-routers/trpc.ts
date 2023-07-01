import { initTRPC } from "@trpc/server"
import { AxiosError } from "axios"
import superjson, { allowErrorProps, registerClass } from "superjson"

import { ActorContext, Reactor } from "@dassie/lib-reactive"

export interface TrpcContext {
  sig: ActorContext
  reactor: Reactor
}

allowErrorProps("stack", "cause")
registerClass(AggregateError, {
  allowProps: ["errors", "message", "stack", "cause"],
})
registerClass(AxiosError, {
  allowProps: ["code", "errors", "message", "name", "config", "cause"],
})

export const trpc = initTRPC.context<TrpcContext>().create({
  transformer: superjson,
})
