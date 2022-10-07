import { initTRPC } from "@trpc/server"
import superjson from "superjson"

import type { ReactiveContext } from "@dassie/lib-reactive-trpc/server"

export const trpc = initTRPC.context<ReactiveContext>().create({
  transformer: superjson,
})
