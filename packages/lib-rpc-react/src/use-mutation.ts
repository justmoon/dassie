import { useMutation } from "@tanstack/react-query"

import type { RpcContextValue } from "./context"
import { getQueryKey } from "./query-key"
import type { HooksRouteSettings } from "./types/hooks-route"
import type { UseMutationHookOptions } from "./types/use-mutation"

export interface CreateUseQueryHookParameters {
  path: string[]
  useRpcContext: () => RpcContextValue
}

export function createUseMutationHook({
  path,
  useRpcContext,
}: CreateUseQueryHookParameters) {
  return function useRpcMutation({
    queryClient: queryClientOptions,
    ...options
  }: UseMutationHookOptions = {}) {
    const { rpcClient, queryClient: queryClientContext } = useRpcContext()
    return useMutation(
      {
        ...options,
        mutationKey: getQueryKey(path, "mutation", undefined),
        mutationFn: (input: HooksRouteSettings["input"]) =>
          rpcClient.mutate(path, input),
      },
      queryClientOptions ?? queryClientContext,
    )
  }
}
