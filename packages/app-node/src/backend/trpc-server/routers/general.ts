import { subscribeToSignal } from "@dassie/lib-reactive-trpc/server"

import { TotalOwnerBalanceSignal } from "../../accounting/computed/total-owner-balance"
import { HasNodeIdentitySignal } from "../../config/computed/has-node-identity"
import { IlpAllocationSchemeSignal } from "../../config/computed/ilp-allocation-scheme"
import { NodeTableStore } from "../../peer-protocol/stores/node-table"
import { ActiveSettlementSchemesSignal } from "../../settlement-schemes/signals/active-settlement-schemes"
import { protectedProcedure } from "../middlewares/auth"
import { trpc } from "../trpc-context"

export const generalRouter = trpc.router({
  getBasicState: trpc.procedure.query(({ ctx: { sig, user } }) => {
    const hasNodeIdentity = sig.use(HasNodeIdentitySignal).read()
    if (!hasNodeIdentity) {
      return {
        state: "uninitialized",
      } as const
    }

    const activeSettlementSchemes = [
      ...sig.use(ActiveSettlementSchemesSignal).read(),
    ]
    const nodeCount = sig.use(NodeTableStore).read().size

    if (!user) {
      return {
        state: "anonymous",
        activeSettlementSchemes,
        nodeCount,
      } as const
    }

    return {
      state: "authenticated",
      activeSettlementSchemes,
      nodeCount,
    } as const
  }),
  subscribeBalance: protectedProcedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, TotalOwnerBalanceSignal)
  }),
  getAllocationScheme: protectedProcedure.query(({ ctx: { sig } }) => {
    return sig.use(IlpAllocationSchemeSignal).read()
  }),
})
