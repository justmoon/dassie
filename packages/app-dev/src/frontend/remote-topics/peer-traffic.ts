import { createRemoteTopic } from "@dassie/lib-reactive-trpc/client"

import { trpcConnectionValue } from "../utils/remote-reactive"

export const peerTrafficTopic = () =>
  createRemoteTopic(trpcConnectionValue, "peerTraffic")
