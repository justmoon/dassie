import { subscribeToSignal } from "@dassie/lib-reactive-trpc/server"

import { ledgerStore } from "../../accounting/stores/ledger"
import { environmentConfigSignal } from "../../config/environment-config"
import { nodeTableStore } from "../../peer-protocol/stores/node-table"
import { routingTableSignal } from "../../routing/signals/routing-table"
import { trpc } from "../trpc-context"

export const debugRouter = trpc.router({
  getLedger: trpc.procedure.query(({ ctx: { sig } }) => {
    return [...sig.use(ledgerStore).getAccounts("")]
  }),
  subscribeConfig: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, environmentConfigSignal)
  }),
  subscribeNodeTable: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, nodeTableStore)
  }),
  subscribeRoutingTable: trpc.procedure.subscription(({ ctx: { sig } }) => {
    return subscribeToSignal(sig, routingTableSignal)
  }),
})
