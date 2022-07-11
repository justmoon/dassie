import type { Component } from "solid-js"
import { For } from "solid-js"

import type { IndexedLogLine } from "../../../backend/features/logs"
import LogLine from "./log-line"

interface LogViewerProperties {
  logs: IndexedLogLine[]
}

const LogViewer: Component<LogViewerProperties> = (properties) => {
  return (
    <div class="flex flex-col-reverse flex-1 h-0 min-h-0 overflow-y-scroll">
      <For each={properties.logs}>{(log) => <LogLine {...log} />}</For>
    </div>
  )
}

export default LogViewer
