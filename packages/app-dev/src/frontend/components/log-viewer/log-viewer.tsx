import type { Component } from "solid-js"
import { For } from "solid-js"

import { logs } from "../../signals/logs"
import LogLine from "./log-line"

const LogViewer: Component = () => {
  return (
    <div class="flex-1 h-0 min-h-0 overflow-y-scroll">
      <For each={logs()}>{(log) => <LogLine {...log} />}</For>
    </div>
  )
}

export default LogViewer
