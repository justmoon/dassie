import { produce } from "immer"
import type { SetOptional } from "type-fest"

import type { IndexedLogLine } from "@dassie/app-node/src/common/stores/logs"
import { createStore } from "@dassie/lib-reactive"

export const LOGS_SOFT_LIMIT = 10_000
export const LOGS_HARD_LIMIT = 10_100

export interface LogsStoreState {
  logs: DevelopmentServerLogLine[]
  startTime: number
}

export interface DevelopmentServerLogLine extends IndexedLogLine {
  node: string
}

type NewLogLine = SetOptional<
  DevelopmentServerLogLine,
  "index" | "relativeTime"
>

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
