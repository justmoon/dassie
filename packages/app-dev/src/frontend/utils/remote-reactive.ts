import { createReactiveHooks } from "@dassie/lib-reactive-trpc/client"

import type { exposedStores } from "../../backend/rpc-routers/remote-reactive-router"

const hooks = createReactiveHooks<typeof exposedStores>()

export const {
  Provider,
  useSig,
  createRemoteTopic,
  createRemoteSignal,
  createRemoteStore,
} = hooks
