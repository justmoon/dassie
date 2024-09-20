import type { ActorContext, Reactor } from "@dassie/lib-reactive"
import { createRoute } from "@dassie/lib-rpc/server"

export const baseRoute = createRoute().context<{
  sig: ActorContext
  reactor: Reactor
}>()
