import { createTrpcConnection } from "@dassie/lib-reactive-trpc/client"

import { client } from "./trpc"

export const trpcConnectionService = () => createTrpcConnection(() => client)
