import { useQuery } from "@tanstack/react-query"

import type { RpcContextValue } from "./context"
import { getQueryKey } from "./query-key"
import type { UseQueryHookOptions } from "./types/use-query"

export interface CreateUseQueryHookParameters {
  path: string[]
  useRpcContext: () => RpcContextValue
}
export function createUseQueryHook({
  path,
  useRpcContext,
}: CreateUseQueryHookParameters) {
  return function useRpcQuery(
    input: unknown,
    { queryClient: queryClientOptions, ...options }: UseQueryHookOptions = {},
  ) {
    const { rpcClient, queryClient: queryClientRpcContext } = useRpcContext()

    return useQuery(
      {
        ...options,
        queryKey: getQueryKey(path, "query", input),
        queryFn: () => rpcClient.query(path, input),
      },
      queryClientOptions ?? queryClientRpcContext,
    )
  }
}
