import { initTRPC } from "@trpc/server"
import superjson from "superjson"

import { TrpcContext } from "./types/trpc-context"

export const trpc = initTRPC
  .context<TrpcContext>()
  .create({ transformer: superjson })
