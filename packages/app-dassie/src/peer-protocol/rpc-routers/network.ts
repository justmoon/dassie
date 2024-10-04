import { subscribeToStore } from "@dassie/lib-reactive-rpc/server"
import { createRouter } from "@dassie/lib-rpc/server"

import { NodeTableStore } from "../../peer-protocol/stores/node-table"
import { protectedRoute } from "../../rpc-server/route-types/protected"

export const networkRouter = createRouter({
  getNodes: protectedRoute.query(({ context: { sig } }) => {
    const nodeTable = sig.reactor.use(NodeTableStore)

    return [...nodeTable.read().values()]
  }),
  subscribeToNodeTableStore: protectedRoute.subscription(
    ({ context: { sig } }) => {
      return subscribeToStore(sig, NodeTableStore)
    },
  ),
})
