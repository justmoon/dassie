import { createTrpcConnectionValue } from "@dassie/lib-reactive-trpc/client"

import { client } from "./trpc"

export const trpcConnectionValue = () => createTrpcConnectionValue(() => client)
