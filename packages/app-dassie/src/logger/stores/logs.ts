import { produce } from "immer"
import type { SetOptional } from "type-fest"

import type { LogMessage } from "@dassie/lib-logger"
import { createStore } from "@dassie/lib-reactive"

export const LOGS_SOFT_LIMIT = 100
export const LOGS_HARD_LIMIT = 120

export type IndexedLogLine = LogMessage & {
  index: number
  relativeTime: number
}

export interface LogsStoreState {
  logs: IndexedLogLine[]
  startTime: number
}

type NewLogLine = SetOptional<IndexedLogLine, "index" | "relativeTime">

export const LogsStore = () => {
  return createStore({
    logs: [],
    startTime: Date.now(),
  } as LogsStoreState).actions({
    clear: () =>
      produce((draft) => {
        draft.logs = []
      }),
    addLogLine: (logLine: NewLogLine) =>
      produce(({ logs, startTime }) => {
        const lastIndex = logs.at(-1)?.index ?? -1

        logs.push({
          index: lastIndex + 1,
          relativeTime: logLine.date - startTime,
          ...logLine,
        })

        if (logs.length > LOGS_HARD_LIMIT) {
          logs.splice(0, logs.length - LOGS_SOFT_LIMIT)
        }
      }),
  } as const)
}
