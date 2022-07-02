import { createSignal } from "solid-js"

import type { NodeLogLine } from "../../topics/log-message"
import client from "../rpc-client"

const [logs, setLogs] = createSignal<NodeLogLine[]>([])

client.subscription("logs", undefined, {
  onNext(logLine) {
    setLogs((logs) => {
      return logLine.type === "data" ? [...logs, logLine.data] : logs
    })
  },
})

export { logs, setLogs }
