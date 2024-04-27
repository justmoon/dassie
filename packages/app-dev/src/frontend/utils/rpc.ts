import superjson, { registerClass } from "superjson"

import {
  type UseWebSocketClientOptions,
  createRpcReact,
} from "@dassie/lib-rpc-react"

import type { UiRpcRouter } from "../../backend/rpc-routers/ui-rpc-router"

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
