import { createRemoteStore } from "@dassie/lib-reactive-trpc/client"

import { trpcConnectionService } from "../utils/remote-reactive"

export const activeTemplate = () =>
  createRemoteStore(trpcConnectionService, "activeTemplate", undefined)
