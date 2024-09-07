/* eslint-disable unicorn/no-negated-condition */
import type { LogEvent } from "./types/log-event"

export const logContextSymbol = Symbol.for("dassie.logger.context")

export interface LogContext {
  captureCaller: boolean

  enableChecker: (component: string) => boolean
  output: (logEvent: LogEvent, context: LogContext) => void
  getCaller: (depth: number, error: Error) => string | undefined

  readonly emit: (logEvent: LogEvent) => void
  readonly realConsole: Console
}

const getGlobalThis = () =>
  (typeof globalThis !== "undefined" ? globalThis
  : typeof global !== "undefined" ? global
  : window) as {
    [logContextSymbol]?: LogContext
  }

const createLogContext = (): LogContext => {
  const context: LogContext = {
    captureCaller: false,

    enableChecker: () => false,
    output: () => void 0,
    getCaller: () => void 0,

    emit: (event) => {
      if (
        event.type === "debug" ? context.enableChecker(event.namespace) : true
      ) {
        context.output(event, context)
      }
    },
    realConsole: Object.assign({}, console),
  }

  return context
}

export const getLogContext = () => {
  const globalThisReference = getGlobalThis()

  if (!globalThisReference[logContextSymbol]) {
    globalThisReference[logContextSymbol] = createLogContext()
  }

  return globalThisReference[logContextSymbol]
}
