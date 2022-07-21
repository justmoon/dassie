import { createSignal } from "solid-js"

import type { IndexedLogLine } from "../../backend/features/logs"
import client from "../rpc-client"

const [logs, setLogs] = createSignal<IndexedLogLine[]>([])

client.subscription("ui.logs", undefined, {
  onNext(logLine) {
    setLogs((logs) => {
      if (logLine.type !== "data") return logs
      if (logLine.data.level === "clear") return []

      return [...logs, logLine.data]
    })
  },
})

export { logs, setLogs }
