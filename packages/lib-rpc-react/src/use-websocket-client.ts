import { useMemo } from "react"

import {
  type Client,
  type ClientOptions,
  type WebSocketLinkOptions,
  createClient,
  createWebSocketLink,
} from "@dassie/lib-rpc/client"
import type { AnyRouter } from "@dassie/lib-rpc/server"

export type UseWebSocketClientHook<TRouter extends AnyRouter> = (
  options: UseWebSocketClientOptions,
) => Client<TRouter>

export interface UseWebSocketClientOptions extends WebSocketLinkOptions {
  clientOptions: Omit<ClientOptions, "connection">
}

export function createUseWebSocketClient<
  TRouter extends AnyRouter,
>(): UseWebSocketClientHook<TRouter> {
  return function useWebSocketClient({
    url,
    clientOptions,
    // eslint-disable-next-line n/no-unsupported-features/node-builtins
    WebSocket = globalThis.WebSocket,
    reconnectDelay,
  }) {
    const websocketLink = useMemo(() => {
      return createWebSocketLink({
        url,
        WebSocket,
        reconnectDelay,
      })
    }, [url, WebSocket, reconnectDelay])

    const client = useMemo(() => {
      return createClient<TRouter>({
        ...clientOptions,
        connection: websocketLink,
      })
    }, [clientOptions, websocketLink])

    return client
  }
}
