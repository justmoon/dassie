import { z } from "zod"

import { createRouter } from "@dassie/lib-rpc/server"

import type { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"
import { protectedRoute } from "../../rpc-server/route-types/protected"
import { SettlementSchemesStore } from "../database-stores/settlement-schemes"
import { ManageSettlementSchemeInstancesActor } from "../manage-settlement-scheme-instances"

export const ledgersRouter = createRouter({
  getList: protectedRoute.query(({ context: { sig } }) => {
    const settlementSchemes = sig.reactor.use(SettlementSchemesStore).read()
    const manageSettlementSchemeInstancesActor = sig.reactor.use(
      ManageSettlementSchemeInstancesActor,
    )

    return Promise.all(
      settlementSchemes.map(async (settlementScheme) => {
        const settlementActor = manageSettlementSchemeInstancesActor.get(
          settlementScheme.id,
        )
        return {
          id: settlementScheme.id,
          balance: (await settlementActor?.api.getBalance.ask()) ?? 0n,
        }
      }),
    )
  }),
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
