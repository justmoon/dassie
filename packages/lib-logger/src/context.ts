import { LogEvent } from "./types/log-event"

/* eslint-disable unicorn/no-negated-condition */
export const logContextSymbol = Symbol.for("dassie.logger.context")

export interface LogContextParameters {
  enableChecker: (component: string) => boolean
  output: (logEvent: LogEvent, context: LogContext) => void
}

export interface LogContext {
  enableChecker: (component: string) => boolean
  output: (logEvent: LogEvent, context: LogContext) => void
  readonly emit: (logEvent: LogEvent) => void
  readonly realConsole: Console
}

const getGlobalThis = () =>
  (typeof globalThis !== "undefined"
    ? globalThis
    : typeof global !== "undefined"
    ? global
    : window) as {
    [logContextSymbol]?: LogContext
  }

const createLogContext = (): LogContext => {
  const context: LogContext = {
    enableChecker: () => false,
    output: () => void 0,
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
