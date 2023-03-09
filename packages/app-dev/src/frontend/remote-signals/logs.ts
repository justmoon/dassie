import { logsStore } from "../../common/stores/logs"
import { createRemoteStore } from "../utils/remote-reactive"

export const remoteLogsStore = () => createRemoteStore("logs", logsStore)
