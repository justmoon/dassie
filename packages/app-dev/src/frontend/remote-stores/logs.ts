import { createRemoteSynchronizedStore } from "@dassie/lib-reactive-trpc/client"

import { logsStore } from "../../common/stores/logs"
import { trpcConnectionValue } from "../utils/remote-reactive"

export const remoteLogsStore = () =>
  createRemoteSynchronizedStore(trpcConnectionValue, "logs", logsStore)
