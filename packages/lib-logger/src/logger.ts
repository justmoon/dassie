import { createEnableChecker } from "./enabled"
import type { Formatter, FormatterConstructor } from "./types/formatter"
import type LogErrorOptions from "./types/log-error-options"

/**
 * Loggers are used to log messages related to a specific component.
 *
 * To create a logger, use the {@link createLogger} function.
 *
 * @beta
 */
export class Logger {
  private readonly formatter: Formatter

  /**
   * Build a new logger object. Use {@link createLogger} instead.
   *
   * @param component - The name of the component or scope.
   * @param Formatter - A formatter that the logger should use for its output.
   * @param debugEnabled - Whether this logger should log debug messages.
   * @internal
   */
  constructor(
    /**
     * An arbitrary name to uniquely identify the component this logger belongs to.
     *
     * @example "xen:node:http"
     */
    readonly component: string,
    Formatter: FormatterConstructor,
    /**
     * Whether this logger should log debug messages.
     */
    readonly debugEnabled: boolean
  ) {
    this.formatter = new Formatter(component)
  }

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
    this.formatter.clear()
  }

  /**
   * This will only log a message if this logger's component is part of the current debug scope.
   *
   * @param message - A freeform message
   * @param data - Additional relevant data to log and make available for debugging
   */
  debug(message: string, data?: Record<string, unknown>) {
    if (this.debugEnabled) {
      this.formatter.log("debug", message, data)
    }
  }

  /**
   * Logs a message.
   *
   * @param message - A freeform message
   * @param data - Additional relevant data to log and make available for debugging
   */
  info(message: string, data?: Record<string, unknown>) {
    this.formatter.log("info", message, data)
  }

  /**
   * Logs a message with extra emphasis.
   *
   * @param message - A freeform message
   * @param data - Additional relevant data to log and make available for debugging
   */
  warn(message: string, data?: Record<string, unknown>) {
    this.formatter.log("warn", message, data)
  }

  /**
   * Logs a message with the greatest possible emphasis.
   *
   * @param message - A freeform message
   * @param data - Additional relevant data to log and make available for debugging
   */
  error(message: string, data?: Record<string, unknown>) {
    this.formatter.log("error", message, data)
  }

  /**
   * Logs an error object with appropriate formatting.
   *
   * @param error - The error which should usually be a JavaScript Error object. However, other types are handled for convenience.
   * @param options - Additional options to control the formatting of the error.
   */
  logError(error: unknown, options: LogErrorOptions = {}) {
    this.formatter.logError(error, options)
  }
}

/**
 * Create a logger factory which applies a given formatter to all loggers.
 *
 * @param debugScope - A comma-separated list of debug scopes to enable. Usually read from the DEBUG environment variable.
 * @param Formatter - The formatter to use for all loggers.
 * @returns A factory function that creates loggers.
 * @internal
 */
export function _createLoggerFactory(
  debugScope: string,
  Formatter: FormatterConstructor
) {
  const enableChecker = createEnableChecker(debugScope)
  const createLogger = (component: string) => {
    return new Logger(component, Formatter, enableChecker(component))
  }
  return createLogger
}
