import { hashKey, useQueryClient } from "@tanstack/react-query"
import { useCallback, useEffect, useMemo } from "react"

import type { RpcContextValue } from "./context"
import { getQueryKey } from "./query-key"
import { getSubscriptionStore } from "./subscription-store"
import type { HooksRouteSettings } from "./types/hooks-route"
import type { UseSubscriptionHookOptions } from "./types/use-subscription"

export interface CreateUseSubscriptionHookParameters {
  path: string[]
  useRpcContext: () => RpcContextValue
}
export function createUseSubscriptionHook({
  path,
  useRpcContext,
}: CreateUseSubscriptionHookParameters) {
  return function useRpcSubscription(
    input: unknown,
    {
      queryClient: queryClientOptions,
      onData,
    }: UseSubscriptionHookOptions<HooksRouteSettings["output"], Error>,
  ) {
    const { rpcClient, queryClient: queryClientRpcContext } = useRpcContext()
    const queryClientContext = useQueryClient()

    const queryClient =
      queryClientOptions ?? queryClientRpcContext ?? queryClientContext

    const subscriptionKey = getQueryKey(path, "subscription", input)
    const subscriptionKeyHash = useMemo(
      () => hashKey(subscriptionKey),
      [subscriptionKey],
    )
    const subscriptionStore = useMemo(
      () => getSubscriptionStore(queryClient),
      [queryClient],
    )

    const handleEvent = useCallback(
      (data: unknown) => {
        onData(data)
      },
      [onData],
    )

    useEffect(() => {
      subscriptionStore.increment(subscriptionKeyHash, () => {
        const disposeSubscription = rpcClient.subscribe(path, input, { onData })

        return disposeSubscription
      })
      return () => {
        subscriptionStore.decrement(subscriptionKeyHash)
      }
    }, [
      subscriptionStore,
      subscriptionKeyHash,
      handleEvent,
      rpcClient,
      input,
      onData,
    ])
  }
}
