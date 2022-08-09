import Denque from "denque"
import { createSignal } from "solid-js"

import type { IndexedLogLine } from "../../backend/features/logs"
import { client } from "../utils/trpc"
import { setGlobalFirehose } from "./global-firehose"

const [logs, setLogs] = createSignal({
  logs: new Denque<IndexedLogLine>(),
})

const MAX_LOG_LINES = 200

client.subscription("logs", undefined, {
  onNext(logLine) {
    setLogs((wrapper) => {
      if (logLine.type !== "data") return wrapper

      const { logs } = wrapper
      if (logLine.data.level === "clear") {
        return { logs: new Denque<IndexedLogLine>() }
      }

      logs.push(logLine.data)

      while (logs.size() > MAX_LOG_LINES) {
        logs.shift()
      }

      return { logs }
    })
    if (logLine.type === "data" && logLine.data.level === "clear") {
      setGlobalFirehose(() => [])
    }
  },
})

export { logs, setLogs }
