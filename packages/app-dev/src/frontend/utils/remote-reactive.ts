import { createRemoteReactiveHooks } from "@xen-ilp/lib-reactive-trpc/client"

import { client } from "./trpc"

const { useLiveRemoteStore } = createRemoteReactiveHooks(client)

export { useLiveRemoteStore }
