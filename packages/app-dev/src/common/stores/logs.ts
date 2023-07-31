import type { SetOptional } from "type-fest"

import type { LogMessage } from "@dassie/lib-logger"
import { createStore } from "@dassie/lib-reactive"

export const LOGS_SOFT_LIMIT = 10_000
export const LOGS_HARD_LIMIT = 10_100

export type NodeLogLine = LogMessage & {
  node: string
}

export type IndexedLogLine = NodeLogLine & {
  index: number
  relativeTime: number
}

type NewLogLine = SetOptional<IndexedLogLine, "index" | "relativeTime">

export const logsStore = () => {
  const startTime = Date.now()

  return createStore(
    [] as IndexedLogLine[],
    {
      // eslint-disable-next-line unicorn/consistent-function-scoping
      clear: () => () => [],
      addLogLine: (logLine: NewLogLine) => (logs) => {
        const lastIndex = logs.at(-1)?.index ?? -1

        logs.push({
          index: lastIndex + 1,
          relativeTime: logLine.date - startTime,
          ...logLine,
        })

        if (logs.length > LOGS_HARD_LIMIT) {
          logs = logs.slice(logs.length - LOGS_SOFT_LIMIT)
        }

        return logs
      },
    } as const
  )
}
