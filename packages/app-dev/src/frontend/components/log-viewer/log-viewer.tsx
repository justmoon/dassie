import { useObserver } from "react-solid-state"

import type { IndexedLogLine } from "../../../backend/features/logs"
import { logs } from "../../signals/logs"
import LogLine from "./log-line"

export interface LogViewerProperties {
  filter?: (line: IndexedLogLine) => boolean
}

const LogViewer = ({ filter }: LogViewerProperties) => {
  return useObserver(() => (
    <div className="flex flex-col-reverse h-full w-full overflow-y-scroll">
      {logs()
        .logs.toArray()
        .filter(filter ?? Boolean)
        .map((line) => (
          <LogLine key={line.index} {...line} />
        ))}
    </div>
  ))
}

export default LogViewer
