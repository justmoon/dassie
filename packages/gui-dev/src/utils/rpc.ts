import superjson, { registerClass } from "superjson"

import type { UiRpcRouter } from "@dassie/app-dev/src/rpc-routers/ui-rpc-router"
import {
  type UseWebSocketClientOptions,
  createRpcReact,
} from "@dassie/lib-rpc-react"

registerClass(TypeError, {
  allowProps: ["message", "stack", "cause"],
})
registerClass(AggregateError, {
  allowProps: ["errors", "message", "stack", "cause"],
})

export const { rpc, RpcProvider, useWebSocketClient } =
  createRpcReact<UiRpcRouter>()

export const clientOptions: UseWebSocketClientOptions["clientOptions"] = {
  transformer: superjson,
}
