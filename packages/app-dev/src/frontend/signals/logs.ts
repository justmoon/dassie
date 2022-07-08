import { createSignal } from "solid-js"

import type { NodeLogLine } from "../../backend/topics/log-message"
import client from "../rpc-client"

const [logs, setLogs] = createSignal<NodeLogLine[]>([])

client.subscription("logs", undefined, {
  onNext(logLine) {
    setLogs((logs) => {
      if (logLine.type !== "data") return logs
      if (logLine.data.level === "clear") return []

      return [...logs, logLine.data]
    })
  },
})

export { logs, setLogs }
