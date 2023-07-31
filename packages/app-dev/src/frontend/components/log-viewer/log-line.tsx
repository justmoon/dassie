import { selectBySeed } from "@dassie/lib-logger"

import type { IndexedLogLine } from "../../../common/stores/logs"
import { COLORS } from "../../constants/palette"
import NodeLink from "../shared/node-link/node-link"
import { LogMessage } from "./log-message"

interface LogLineProperties {
  log: IndexedLogLine
}

const LOG_LEVEL_COLORS = {
  debug: "#7e84fa",
  info: "#147af3",
  warn: "#e8c600",
  error: "#de3d82",
} as const

const LogLine = ({ log }: LogLineProperties) => {
  return (
    <div
      className="flex space-x-1 text-xs py-0.5 items-baseline"
      style={{ order: -log.index }}
    >
      <div className="font-mono flex-shrink-0 text-right px-2 text-gray-400 w-20">
        {(log.relativeTime / 1000).toFixed(3)}
      </div>
      <div className="w-10" style={{ color: LOG_LEVEL_COLORS[log.type] }}>
        {log.type}
      </div>
      <div className="font-bold flex-shrink-0 w-8">
        <NodeLink id={log.node} />
      </div>
      <pre className="font-mono px-2 whitespace-pre-wrap">
        <span style={{ color: selectBySeed(COLORS, log.namespace) }}>
          {log.namespace}
        </span>{" "}
        <LogMessage message={log.message} parameters={log.parameters} />
      </pre>
    </div>
  )
}

export default LogLine
