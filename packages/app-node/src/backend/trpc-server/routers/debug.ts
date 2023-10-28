import { subscribeToSignal } from "@dassie/lib-reactive-trpc/server"

import { LedgerStore } from "../../accounting/stores/ledger"
import { EnvironmentConfigSignal } from "../../config/environment-config"
import { NodeTableStore } from "../../peer-protocol/stores/node-table"
import { RoutingTableSignal } from "../../routing/signals/routing-table"
import { protectedProcedure } from "../middlewares/auth"
import { trpc } from "../trpc-context"

export const debugRouter = trpc.router({
  getLedger: protectedProcedure.query(({ ctx: { sig } }) => {
    return [...sig.use(LedgerStore).getAccounts("")]
  }),
  subscribeConfig: protectedProcedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, EnvironmentConfigSignal)
  }),
  subscribeNodeTable: protectedProcedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, NodeTableStore)
  }),
  subscribeRoutingTable: protectedProcedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, RoutingTableSignal)
  }),
})
