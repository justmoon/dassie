import { subscribeToSignal } from "@dassie/lib-reactive-trpc/server"

import { totalOwnerBalanceComputed } from "../../accounting/computed/total-owner-balance"
import { ilpAllocationSchemeSignal } from "../../config/computed/ilp-allocation-scheme"
import { protectedProcedure } from "../middlewares/auth"
import { trpc } from "../trpc-context"

export const generalRouter = trpc.router({
  subscribeBalance: protectedProcedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, totalOwnerBalanceComputed)
  }),
  getAllocationScheme: protectedProcedure.query(({ ctx: { sig } }) => {
    return sig.use(ilpAllocationSchemeSignal).read()
  }),
})
