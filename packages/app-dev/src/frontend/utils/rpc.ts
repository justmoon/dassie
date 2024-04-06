import { AxiosError } from "axios"
import superjson, { registerClass } from "superjson"

import {
  type UseWebSocketClientOptions,
  createRpcReact,
} from "@dassie/lib-rpc-react"

import type { AppRouter } from "../../backend/rpc-routers/app-router"

registerClass(AggregateError, {
  allowProps: ["errors", "message", "stack", "cause"],
})
registerClass(AxiosError, {
  allowProps: ["code", "errors", "message", "name", "config", "cause"],
})

export const { rpc, RpcProvider, useWebSocketClient } =
  createRpcReact<AppRouter>()

export const clientOptions: UseWebSocketClientOptions["clientOptions"] = {
  transformer: superjson,
}
