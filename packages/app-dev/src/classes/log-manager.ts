import { produce } from "immer"

import type { EventBroker } from "@xen-ilp/lib-events"
import type { State, Store } from "@xen-ilp/lib-state"

import { NodeLogLine, logLineTopic } from "../topics/log-message"

export interface LogLine {
  message: string
}

export interface LogManagerContext {
  eventBroker: EventBroker
  state: State
}

export type Model = NodeLogLine[]

export default class LogManager {
  readonly store: Store<Model>

  constructor(readonly context: LogManagerContext) {
    this.store = this.context.state.createStore<Model>("log-manager", [])

    this.context.eventBroker.addListener(logLineTopic, this.addLogLine)
  }

  addLogLine = (logLine: NodeLogLine) => {
    this.store.set(
      produce((draft) => {
        if (
          logLine.component === "xen:logger" &&
          logLine.message === "%%clear%%"
        ) {
          draft.splice(0, draft.length)
        } else {
          draft.push(logLine)
        }
      })
    )
  }
}
