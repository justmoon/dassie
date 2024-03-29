import { subscribeToSignal } from "@dassie/lib-reactive-trpc/server"

import { TotalOwnerBalanceSignal } from "../../accounting/computed/total-owner-balance"
import { HasNodeIdentitySignal } from "../../config/computed/has-node-identity"
import { IlpAllocationSchemeSignal } from "../../config/computed/ilp-allocation-scheme"
import { DatabaseConfigStore } from "../../config/database-config"
import { NodeIdSignal } from "../../ilp-connector/computed/node-id"
import { NodeTableStore } from "../../peer-protocol/stores/node-table"
import { ActiveSettlementSchemesSignal } from "../../settlement-schemes/signals/active-settlement-schemes"
import { protectedProcedure } from "../middlewares/auth"
import { trpc } from "../trpc-context"

export const generalRouter = trpc.router({
  getBasicState: trpc.procedure.query(({ ctx: { sig, user } }) => {
    const hasNodeIdentity = sig.read(HasNodeIdentitySignal)
    if (!hasNodeIdentity) {
      return {
        state: "uninitialized",
      } as const
    }

    const activeSettlementSchemes = [...sig.read(ActiveSettlementSchemesSignal)]
    const nodeCount = sig.read(NodeTableStore).size

    const hostname = sig.read(DatabaseConfigStore).hostname

    const nodeId = sig.read(NodeIdSignal)

    // Please note that this is a public method that anyone can call so don't
    // return any non-public information here.
    return {
      state: user ? "authenticated" : "anonymous",
      activeSettlementSchemes,
      nodeCount,
      hostname,
      nodeId,
    } as const
  }),
  subscribeBalance: protectedProcedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, TotalOwnerBalanceSignal)
  }),
  getAllocationScheme: protectedProcedure.query(({ ctx: { sig } }) => {
    return sig.read(IlpAllocationSchemeSignal)
  }),
})
