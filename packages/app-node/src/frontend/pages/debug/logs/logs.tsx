import { useRemoteStore } from "@dassie/lib-reactive-trpc/client"

import { LogsStore } from "../../../../common/stores/logs"
import LogViewer from "../../../components/log-viewer/log-viewer"
import { trpc } from "../../../utils/trpc"

export function Logs() {
  const { logs } = useRemoteStore(trpc.debug.subscribeToLogs, LogsStore)

  return <LogViewer logs={logs} />
}
