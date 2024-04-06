import { RpcFailure } from "@dassie/lib-rpc/server"

import { publicRoute } from "./public"

export const protectedRoute = publicRoute.use(
  ({ context: { isAuthenticated } }) => {
    if (!isAuthenticated) {
      return new RpcFailure("Unauthorized")
    }

    return
  },
)
