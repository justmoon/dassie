import assert from "node:assert"

import { createEnableChecker } from "./enabled"
import type { LoggingContext } from "./types/context"

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
     * @example "das:node:http"
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
    console.clear()
  }

  /**
   * This will only log a message if this logger's component is part of the current debug scope.
   *
   * @param message - A freeform message
   * @param parameters - Additional relevant data to log and make available for debugging
   */
  debug(message: string, ...parameters: unknown[]) {
    if (this.context.enableChecker(this.component)) {
      console.debug(`${this.component} ${message}`, ...parameters)
    }
  }

  /**
   * Logs a message.
   *
   * @param message - A freeform message
   * @param parameters - Additional relevant data to log and make available for debugging
   */
  info(message: string, ...parameters: unknown[]) {
    console.info(`${this.component} ${message}`, ...parameters)
  }

  /**
   * Logs a message with extra emphasis.
   *
   * @param message - A freeform message
   * @param parameters - Additional relevant data to log and make available for debugging
   */
  warn(message: string, ...parameters: unknown[]) {
    console.warn(`${this.component} ${message}`, ...parameters)
  }

  /**
   * Logs a message with the greatest possible emphasis.
   *
   * @param message - A freeform message
   * @param parameters - Additional relevant data to log and make available for debugging
   */
  error(message: string, ...parameters: unknown[]) {
    console.error(`${this.component} ${message}`, ...parameters)
  }
}

/**
 * Create a logger factory which creates loggers using a given logging context.
 *
 * @alpha
 */
export function createLoggerFactory(loggingContext: LoggingContext) {
  const createLogger = (component: string) => {
    assert(!component.includes(" "), "Component name must not contain spaces")

    return new Logger(loggingContext, component)
  }

  createLogger.setDebugScope = (debugScope: string) => {
    loggingContext.enableChecker = createEnableChecker(debugScope)
  }

  return createLogger
}
