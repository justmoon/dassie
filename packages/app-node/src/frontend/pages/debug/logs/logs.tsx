import { useRemoteStore } from "@dassie/lib-reactive-rpc/client"

import { LogsStore } from "../../../../common/stores/logs"
import LogViewer from "../../../components/log-viewer/log-viewer"
import { rpc } from "../../../utils/rpc"

export function Logs() {
  const { logs } = useRemoteStore(rpc.debug.subscribeToLogs, LogsStore)

  return <LogViewer logs={logs} />
}
