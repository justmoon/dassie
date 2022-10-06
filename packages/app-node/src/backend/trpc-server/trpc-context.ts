import type { EffectContext } from "@dassie/lib-reactive"

export const createContextFactory = (sig: EffectContext) => () => {
  return {
    sig,
  }
}

export type TrpcContext = ReturnType<ReturnType<typeof createContextFactory>>
