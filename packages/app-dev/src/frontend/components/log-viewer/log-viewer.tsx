import type { Component } from "solid-js"
import { For } from "solid-js"

import { logs } from "../../signals/logs"
import LogLine from "./log-line"

const LogViewer: Component = () => {
  return <For each={logs()}>{(log) => <LogLine {...log} />}</For>
}

export default LogViewer
