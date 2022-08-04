import { isObject } from "@xen-ilp/lib-type-utils"

import { createEnableChecker } from "./enabled"
import type { LoggingContext } from "./types/context"
import type { Formatter } from "./types/formatter"
import type { LogLineOptions } from "./types/log-options"

/**
 * Loggers are used to log messages related to a specific component.
 *
 * To create a logger, use the {@link createLogger} function.
 *
 * @beta
 */
export class Logger {
  /**
   * Build a new logger object. Use {@link createLogger} instead.
   *
   * @param context - Object specifying the environment that the logger should interact with, e.g. the formatter
   * @param component - The name of the component or scope.
   * @internal
   */
  constructor(
    readonly context: LoggingContext,

    /**
     * An arbitrary name to uniquely identify the component this logger belongs to.
     *
     * @example "xen:node:http"
     */
    readonly component: string
  ) {}

  /**
   * Clears the logger's output.
   *
   * @remarks
   *
   * For example, if the logger is used to log to the console, this will clear the console.
   *
   * @alpha
   */
  clear() {
    this.context.formatter(
      {
        component: this.component,
        date: new Date(),
        level: "clear",
        message: "",
      },
      {}
    )
  }

  /**
   * This will only log a message if this logger's component is part of the current debug scope.
   *
   * @param message - A freeform message
   * @param data - Additional relevant data to log and make available for debugging
   */
  debug(
    message: string,
    data?: Record<string, unknown>,
    options?: LogLineOptions
  ) {
    if (this.context.enableChecker(this.component)) {
      this.context.formatter(
        {
          component: this.component,
          date: new Date(),
          level: "debug",
          message,
          ...(data ? { data } : {}),
        },
        options ?? {}
      )
    }
  }

  /**
   * Logs a message.
   *
   * @param message - A freeform message
   * @param data - Additional relevant data to log and make available for debugging
   */
  info(
    message: string,
    data?: Record<string, unknown>,
    options?: LogLineOptions
  ) {
    this.context.formatter(
      {
        component: this.component,
        date: new Date(),
        level: "info",
        message,
        ...(data ? { data } : {}),
      },
      options ?? {}
    )
  }

  /**
   * Logs a message with extra emphasis.
   *
   * @param message - A freeform message
   * @param data - Additional relevant data to log and make available for debugging
   */
  warn(
    message: string,
    data?: Record<string, unknown>,
    options?: LogLineOptions
  ) {
    this.context.formatter(
      {
        component: this.component,

        date: new Date(),
        level: "warn",
        message,
        ...(data ? { data } : {}),
      },
      options ?? {}
    )
  }

  /**
   * Logs a message with the greatest possible emphasis.
   *
   * @param message - A freeform message
   * @param data - Additional relevant data to log and make available for debugging
   */
  error(
    message: string,
    data?: Record<string, unknown>,
    options?: LogLineOptions
  ) {
    this.context.formatter(
      {
        component: this.component,
        date: new Date(),
        level: "error",
        message,
        ...(data ? { data } : {}),
      },
      options ?? {}
    )
  }

  captureConsole() {
    const methods = ["debug", "info", "warn", "error"] as const

    for (const method of methods) {
      console[method] = (message: string, ...parameters: unknown[]) => {
        this[method](
          message,
          parameters.length === 1 && isObject(parameters[0])
            ? parameters[0] ?? undefined
            : { parameters }
        )
      }
    }
  }
}

/**
 * Create a logger factory which creates loggers using a given logging context.
 *
 * @alpha
 */
export function createLoggerFactory(loggingContext: LoggingContext) {
  const createLogger = (component: string) => {
    return new Logger(loggingContext, component)
  }

  createLogger.setDebugScope = (debugScope: string) => {
    loggingContext.enableChecker = createEnableChecker(debugScope)
  }

  createLogger.setFormatter = (formatter: Formatter) => {
    loggingContext.formatter = formatter
  }

  return createLogger
}
