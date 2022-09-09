import { createRemoteTopic } from "@dassie/lib-reactive-trpc/client"

import { trpcConnectionService } from "../utils/remote-reactive"

export const peerTrafficTopic = () =>
  createRemoteTopic(trpcConnectionService, "peerTraffic")
