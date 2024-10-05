import { subscribeToSignal } from "@dassie/lib-reactive-rpc/server"
import { createRouter } from "@dassie/lib-rpc/server"

import { TotalOwnerBalanceSignal } from "../../accounting/computed/total-owner-balance"
import { HasNodeIdentitySignal } from "../../config/computed/has-node-identity"
import { IlpAllocationSchemeSignal } from "../../config/computed/ilp-allocation-scheme"
import { DatabaseConfigStore } from "../../config/database-config"
import { NodeIdSignal } from "../../ilp-connector/computed/node-id"
import { ActiveSettlementSchemesSignal } from "../../ledgers/signals/active-settlement-schemes"
import { NodeTableStore } from "../../peer-protocol/stores/node-table"
import { protectedRoute } from "../route-types/protected"
import { publicRoute } from "../route-types/public"

export const generalRouter = createRouter({
  getBasicState: publicRoute.query(({ context: { sig, isAuthenticated } }) => {
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
    const ilpAllocationScheme = sig.read(IlpAllocationSchemeSignal)

    // Please note that this is a public method that anyone can call so don't
    // return any non-public information here.
    return {
      state: isAuthenticated ? "authenticated" : "anonymous",
      activeSettlementSchemes,
      nodeCount,
      hostname,
      nodeId,
      ilpAllocationScheme,
    } as const
  }),
  subscribeBalance: protectedRoute.subscription(({ context: { sig } }) => {
    return subscribeToSignal(sig, TotalOwnerBalanceSignal)
  }),
})
