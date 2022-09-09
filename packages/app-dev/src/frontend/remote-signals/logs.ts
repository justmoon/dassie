import { createRemoteSynchronizedStore } from "@dassie/lib-reactive-trpc/client"

import { logsStore } from "../../common/stores/logs"
import { trpcConnectionService } from "../utils/remote-reactive"

export const remoteLogsStore = () =>
  createRemoteSynchronizedStore(trpcConnectionService, "logs", logsStore)
