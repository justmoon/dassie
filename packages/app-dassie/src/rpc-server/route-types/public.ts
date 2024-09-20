import { createRoute } from "@dassie/lib-rpc/server"

import type { DassieActorContext } from "../../base/types/dassie-base"

export const publicRoute = createRoute().context<{
  sig: DassieActorContext
  isAuthenticated: boolean
}>()
