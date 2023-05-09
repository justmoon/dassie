import type { SetOptional } from "type-fest"

import type { SerializableLogLine } from "@dassie/lib-logger"
import { createStore } from "@dassie/lib-reactive"

export const LOGS_SOFT_LIMIT = 10_000
export const LOGS_HARD_LIMIT = 10_100

export interface NodeLogLine extends SerializableLogLine {
  node: string
}

export interface IndexedLogLine extends NodeLogLine {
  index: number
  relativeTime: number
}

type NewLogLine = SetOptional<IndexedLogLine, "index" | "relativeTime">

export const logsStore = () =>
  createStore(
    { logs: [] as IndexedLogLine[] } as const,
    {
      // eslint-disable-next-line unicorn/consistent-function-scoping
      clear: () => () => ({
        logs: [],
      }),
      addLogLine:
        (logLine: NewLogLine) =>
        ({ logs }) => {
          const lastIndex = logs.at(-1)?.index ?? -1
          const startTime = new Date(logs[0]?.date ?? new Date().toISOString())

          logs.push({
            index: lastIndex + 1,
            relativeTime: Date.now() - Number(startTime),
            ...logLine,
          })

          if (logs.length > LOGS_HARD_LIMIT) {
            logs = logs.slice(logs.length - LOGS_SOFT_LIMIT)
          }

          return { logs }
        },
    } as const
  )
