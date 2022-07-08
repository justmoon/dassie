import type { Component } from "solid-js"
import { For } from "solid-js"

import { logs } from "../../signals/logs"
import LogLine from "./log-line"

const LogViewer: Component = () => {
  return (
    <div class="flex flex-col flex-1 h-0 py-8 px-4 overflow-y-auto sm:px-0">
      <For each={logs()}>{(log) => <LogLine {...log} />}</For>
    </div>
  )
}

export default LogViewer
