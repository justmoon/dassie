import { z } from "zod"

import { createRouter } from "@dassie/lib-rpc/server"

import type { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"
import { protectedRoute } from "../../rpc-server/route-types/protected"
import { ManageSettlementSchemeInstancesActor } from "../manage-settlement-scheme-instances"

export const settlementRouter = createRouter({
  stubDeposit: protectedRoute
    .input(z.string())
    .mutation(async ({ input: amount, context: { sig } }) => {
      const manageSettlementSchemeInstancesActor = sig.reactor.use(
        ManageSettlementSchemeInstancesActor,
      )

      const stubActor = manageSettlementSchemeInstancesActor.get(
        "stub" as SettlementSchemeId,
      )

      if (!stubActor) {
        throw new Error("Stub settlement scheme is not enabled")
      }

      await stubActor.api.handleDeposit.ask({ amount: BigInt(amount) })

      return true
    }),
})
