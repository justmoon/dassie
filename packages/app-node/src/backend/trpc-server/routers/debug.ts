import { subscribeToSignal } from "@dassie/lib-reactive-trpc/server"

import { ledgerStore } from "../../accounting/stores/ledger"
import { environmentConfigSignal } from "../../config/environment-config"
import { nodeTableStore } from "../../peer-protocol/stores/node-table"
import { routingTableSignal } from "../../routing/signals/routing-table"
import { protectedProcedure } from "../middlewares/auth"
import { trpc } from "../trpc-context"

export const debugRouter = trpc.router({
  getLedger: protectedProcedure.query(({ ctx: { sig } }) => {
    return [...sig.use(ledgerStore).getAccounts("")]
  }),
  subscribeConfig: protectedProcedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, environmentConfigSignal)
  }),
  subscribeNodeTable: protectedProcedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, nodeTableStore)
  }),
  subscribeRoutingTable: protectedProcedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, routingTableSignal)
  }),
})
