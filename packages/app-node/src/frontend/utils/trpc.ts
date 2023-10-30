import { QueryClient } from "@tanstack/react-query"
import { createTRPCReact } from "@trpc/react-query"
import { inferRouterInputs, inferRouterOutputs } from "@trpc/server"
import { createContext } from "react"

import type { AppRouter } from "../../backend/trpc-server/app-router"

const trpcReactContext = createContext<null>(null)
export const queryClientReactContext = createContext<QueryClient | undefined>(
  undefined,
)

export const trpc = createTRPCReact<AppRouter>({
  context: trpcReactContext,
  reactQueryContext: queryClientReactContext,
})

export type RouterInput = inferRouterInputs<AppRouter>
export type RouterOutput = inferRouterOutputs<AppRouter>
