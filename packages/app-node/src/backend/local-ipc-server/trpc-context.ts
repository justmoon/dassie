import { initTRPC } from "@trpc/server"

import { ActorContext } from "@dassie/lib-reactive"

export interface IpcContext {
  sig: ActorContext
}

export const trpc = initTRPC.context<IpcContext>().create()
