import { z } from "zod"

import { trpc } from "../../local-ipc-server/trpc-context"
import { SettlementSchemeId } from "../../peer-protocol/types/settlement-scheme-id"
import { protectedProcedure } from "../../trpc-server/middlewares/auth"
import { ManageSettlementSchemeInstancesActor } from "../manage-settlement-scheme-instances"

export const settlementRouter = trpc.router({
  stubDeposit: protectedProcedure
    .input(z.string())
    .mutation(async ({ input: amount, ctx: { sig } }) => {
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
