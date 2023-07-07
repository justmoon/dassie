import { subscribeToSignal } from "@dassie/lib-reactive-trpc/server"

import { totalOwnerBalanceComputed } from "../../accounting/computed/total-owner-balance"
import { ilpAllocationSchemeSignal } from "../../config/computed/ilp-allocation-scheme"
import { databaseConfigSignal } from "../../config/database-config"
import { protectedProcedure } from "../middlewares/auth"
import { trpc } from "../trpc-context"

export const generalRouter = trpc.router({
  getBasicState: trpc.procedure.query(({ ctx: { sig, user } }) => {
    const config = sig.use(databaseConfigSignal).read()

    if (!config.hasNodeIdentity) {
      return {
        state: "uninitialized",
      } as const
    }

    if (!user) {
      return {
        state: "anonymous",
      } as const
    }

    return {
      state: "authenticated",
    } as const
  }),
  subscribeBalance: protectedProcedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, totalOwnerBalanceComputed)
  }),
  getAllocationScheme: protectedProcedure.query(({ ctx: { sig } }) => {
    return sig.use(ilpAllocationSchemeSignal).read()
  }),
})
