import { useStore } from "@dassie/lib-reactive-react"

import type { IndexedLogLine } from "../../../backend/features/logs"
import { remoteLogsStore } from "../../remote-stores/logs"
import LogLine from "./log-line"

export interface LogViewerProperties {
  filter?: (line: IndexedLogLine) => boolean
}

const LogViewer = ({ filter }: LogViewerProperties) => {
  const logs = useStore(remoteLogsStore)?.logs ?? []
  const renderedLogs = []

  for (const line of logs) {
    if (filter && !filter(line)) continue

    renderedLogs.push(<LogLine key={line.index} {...line} />)
  }

  return (
    <div className="flex flex-col-reverse h-full w-full overflow-y-scroll">
      {renderedLogs}
    </div>
  )
}

export default LogViewer
