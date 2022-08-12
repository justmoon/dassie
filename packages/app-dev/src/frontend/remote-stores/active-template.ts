import { createRemoteStore } from "@dassie/lib-reactive-trpc/client"

import { trpcConnectionValue } from "../utils/remote-reactive"

export const activeTemplate = () =>
  createRemoteStore(trpcConnectionValue, "activeTemplate", undefined)
