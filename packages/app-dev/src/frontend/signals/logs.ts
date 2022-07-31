import { createSignal } from "solid-js"

import type { IndexedLogLine } from "../../backend/features/logs"
import { client } from "../utils/trpc"
import { setGlobalFirehose } from "./global-firehose"

const [logs, setLogs] = createSignal<IndexedLogLine[]>([])

client.subscription("ui.logs", undefined, {
  onNext(logLine) {
    setLogs((logs) => {
      if (logLine.type !== "data") return logs
      if (logLine.data.level === "clear") return []

      return [...logs, logLine.data]
    })
    if (logLine.type === "data" && logLine.data.level === "clear") {
      setGlobalFirehose(() => [])
    }
  },
})

export { logs, setLogs }
