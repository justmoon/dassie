import type { ActorContext } from "@dassie/lib-reactive"

export const createContextFactory = (sig: ActorContext) => () => {
  return {
    sig,
  }
}

export type TrpcContext = ReturnType<ReturnType<typeof createContextFactory>>
