import { subscribeToSignal } from "@dassie/lib-reactive-trpc/server"

import { totalOwnerBalanceComputed } from "../../accounting/computed/total-owner-balance"
import { ilpAllocationSchemeSignal } from "../../config/computed/ilp-allocation-scheme"
import { trpc } from "../trpc-context"

export const generalRouter = trpc.router({
  subscribeBalance: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, totalOwnerBalanceComputed)
  }),
  getAllocationScheme: trpc.procedure.query(({ ctx: { sig } }) => {
    return sig.use(ilpAllocationSchemeSignal).read()
  }),
})
