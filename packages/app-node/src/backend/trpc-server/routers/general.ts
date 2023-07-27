import { subscribeToSignal } from "@dassie/lib-reactive-trpc/server"

import { totalOwnerBalanceComputed } from "../../accounting/computed/total-owner-balance"
import { hasNodeIdentityComputed } from "../../config/computed/has-node-identity"
import { ilpAllocationSchemeSignal } from "../../config/computed/ilp-allocation-scheme"
import { activeSettlementSchemesSignal } from "../../settlement-schemes/signals/active-settlement-schemes"
import { protectedProcedure } from "../middlewares/auth"
import { trpc } from "../trpc-context"

export const generalRouter = trpc.router({
  getBasicState: trpc.procedure.query(({ ctx: { sig, user } }) => {
    const hasNodeIdentity = sig.use(hasNodeIdentityComputed).read()
    if (!hasNodeIdentity) {
      return {
        state: "uninitialized",
      } as const
    }

    const activeSettlementSchemes = sig
      .use(activeSettlementSchemesSignal)
      .read()

    if (!user) {
      return {
        state: "anonymous",
        activeSettlementSchemes,
      } as const
    }

    return {
      state: "authenticated",
      activeSettlementSchemes,
    } as const
  }),
  subscribeBalance: protectedProcedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, totalOwnerBalanceComputed)
  }),
  getAllocationScheme: protectedProcedure.query(({ ctx: { sig } }) => {
    return sig.use(ilpAllocationSchemeSignal).read()
  }),
})
