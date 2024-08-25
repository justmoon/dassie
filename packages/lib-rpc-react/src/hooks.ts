import { createRecursiveProxy } from "@dassie/lib-rpc/client"
import type { AnyRoute, AnyRouter } from "@dassie/lib-rpc/server"

import { type RpcContext, createRpcContext } from "./context"
import type { HooksRoute } from "./types/hooks-route"
import type { UseMutationHookOptions } from "./types/use-mutation"
import type { UseQueryHookOptions } from "./types/use-query"
import type { UseSubscriptionHookOptions } from "./types/use-subscription"
import { createUseMutationHook } from "./use-mutation"
import { createUseQueryHook } from "./use-query"
import { createUseSubscriptionHook } from "./use-subscription"
import {
  type UseWebSocketClientHook,
  createUseWebSocketClient,
} from "./use-websocket-client"

export interface RpcHooks<TRouter extends AnyRouter>
  extends RpcContext<TRouter> {
  useWebSocketClient: UseWebSocketClientHook<TRouter>
  rpc: DeriveHooksRouter<TRouter>
}

export type DeriveHooksRouter<TRouter extends AnyRouter> = {
  [K in keyof TRouter["routes"]]: DeriveHooksRoute<TRouter["routes"][K]>
}

export type DeriveHooksRoute<TRoute> =
  TRoute extends AnyRoute ?
    HooksRoute<{
      type: TRoute["type"]
      input: Parameters<TRoute>[0]["input"]
      output: Awaited<ReturnType<TRoute>>
    }>
  : TRoute extends AnyRouter ? DeriveHooksRouter<TRoute>
  : never

export function createRpcReact<TRouter extends AnyRouter>(): RpcHooks<TRouter> {
  const { RpcProvider, useRpcContext: useRpcContext } =
    createRpcContext<TRouter>()

  const useWebSocketClient = createUseWebSocketClient<TRouter>()

  const rpc = createRecursiveProxy(({ path, parameters }) => {
    const hookName = path.pop()

    switch (hookName) {
      case "useQuery": {
        const [input, options] = parameters as [unknown, UseQueryHookOptions]
        return createUseQueryHook({ path, useRpcContext })(input, options)
      }
      case "useMutation": {
        const [options] = parameters as [UseMutationHookOptions]
        return createUseMutationHook({ path, useRpcContext })(options)
      }
      case "useSubscription": {
        const [input, options] = parameters as [
          unknown,
          UseSubscriptionHookOptions,
        ]
        // eslint-disable-next-line @typescript-eslint/no-confusing-void-expression
        return createUseSubscriptionHook({ path, useRpcContext })(
          input,
          options,
        )
      }
      default: {
        throw new Error(
          hookName ?
            `Invalid hook name: ${hookName}`
          : "You cannot call the root of the RPC object",
        )
      }
    }
  }) as DeriveHooksRouter<TRouter>
  return {
    RpcProvider,
    useWebSocketClient,
    useRpcContext,
    rpc,
  }
}
