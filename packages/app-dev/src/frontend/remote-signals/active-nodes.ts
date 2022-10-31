import { createRemoteSignal } from "@dassie/lib-reactive-trpc/client"

import { trpcConnectionService } from "../utils/remote-reactive"

export const activeNodesStore = () =>
  createRemoteSignal(trpcConnectionService, "activeNodes", undefined)
