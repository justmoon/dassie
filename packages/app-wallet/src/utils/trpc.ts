import { createTRPCReact } from "@trpc/react"

import type { AppRouter } from "../../../app-node/src/trpc-server/app-router"

export const trpc = createTRPCReact<AppRouter>()
