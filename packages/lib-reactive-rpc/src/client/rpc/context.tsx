import type { QueryClient } from "@tanstack/react-query"
import { type ReactNode, createContext, useContext } from "react"

import type { Client } from "@dassie/lib-rpc/client"
import type { AnyRouter } from "@dassie/lib-rpc/server"

export type RpcContext<TRouter extends AnyRouter> = ReturnType<
  typeof createRpcContext<TRouter>
>

export interface RpcContextValue<TRouter extends AnyRouter = AnyRouter> {
  rpcClient: Client<TRouter>
  queryClient?: QueryClient | undefined
}

export interface RpcProviderProperties<TRouter extends AnyRouter> {
  rpcClient: Client<TRouter>
  queryClient?: QueryClient | undefined
  children: ReactNode
}

export function createRpcContext<TRouter extends AnyRouter>() {
  const RpcContext = createContext<null | RpcContextValue<TRouter>>(null)

  const RpcProvider = ({
    rpcClient,
    queryClient,
    children,
  }: RpcProviderProperties<TRouter>) => (
    <RpcContext.Provider value={{ rpcClient, queryClient }}>
      {children}
    </RpcContext.Provider>
  )

  const useRpcContext = () => {
    const value = useContext(RpcContext)

    if (value === null) {
      throw new Error("useRpcClient must be used within a <RpcProvider>")
    }

    return value
  }

  return {
    RpcProvider,
    useRpcContext,
  }
}
