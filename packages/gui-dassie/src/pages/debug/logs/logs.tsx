import { LogsStore } from "@dassie/app-dassie/src/logger/stores/logs"
import { useRemoteStore } from "@dassie/lib-reactive-rpc/client"

import LogViewer from "../../../components/log-viewer/log-viewer"
import { rpc } from "../../../utils/rpc"

export function Logs() {
  const { logs } = useRemoteStore(rpc.debug.subscribeToLogs, LogsStore)

  return <LogViewer logs={logs} />
}
