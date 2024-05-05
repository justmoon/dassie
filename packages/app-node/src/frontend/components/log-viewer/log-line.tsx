import type { IndexedLogLine } from "../../../common/stores/logs"
import { LogMessage } from "./log-message"
import { LogNamespace } from "./log-namespace"
import { useLogViewerContext } from "./log-viewer"

interface LogLineProperties {
  log: ViewableLogLine
}

export interface ViewableLogLine extends IndexedLogLine {
  node?: string
}

const LOG_LEVEL_COLORS = {
  debug: "#7e84fa",
  info: "#147af3",
  warn: "#e8c600",
  error: "#de3d82",
} as const

const LogLine = ({ log }: LogLineProperties) => {
  const { renderNodeColumn } = useLogViewerContext()
  return (
    <div
      className="text-xs whitespace-pre-wrap pl-42"
      style={{ order: -log.index }}
    >
      <span className="inline-block font-mono flex-shrink-0 text-right text-gray-400 w-20 -ml-42 mr-1">
        {(log.relativeTime / 1000).toFixed(3)}
      </span>
      <span
        className="inline-block w-10 mr-1"
        style={{ color: LOG_LEVEL_COLORS[log.type] }}
      >
        {" "}
        {log.type}
      </span>
      {renderNodeColumn?.(log)}
      <span className="font-mono">
        <LogNamespace namespace={log.namespace} caller={log.caller} />{" "}
        <LogMessage message={log.message} parameters={log.parameters} />
      </span>
    </div>
  )
}

export default LogLine
