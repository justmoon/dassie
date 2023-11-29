/* eslint-disable unicorn/error-message */
import { LogContext, getLogContext } from "./context"

/**
 * Loggers are used to log messages related to a specific component.
 *
 * To create a logger, use the {@link createLogger} function.
 *
 * @beta
 */
export interface Logger {
  /**
   * Clears the logger's output.
   *
   * @remarks
   *
   * For example, if the logger is used to log to the console, this will clear the console.
   *
   * @alpha
   */
  clear(): void

  /**
   * This will only log a message if this logger's component is part of the current debug scope.
   *
   * @param message - A freeform message
   * @param parameters - Additional relevant data to log and make available for debugging
   */
  debug(message: string, ...parameters: unknown[]): void

  /**
   * Logs a message.
   *
   * @param message - A freeform message
   * @param parameters - Additional relevant data to log and make available for debugging
   */
  info(message: string, ...parameters: unknown[]): void

  /**
   * Logs a message with extra emphasis.
   *
   * @param message - A freeform message
   * @param parameters - Additional relevant data to log and make available for debugging
   */
  warn(message: string, ...parameters: unknown[]): void

  /**
   * Logs a message with the greatest possible emphasis.
   *
   * @param message - A freeform message
   * @param parameters - Additional relevant data to log and make available for debugging
   */
  error(message: string, ...parameters: unknown[]): void
}

export class LoggerImplementation implements Logger {
  /**
   * Build a new logger object. Use {@link createLogger} instead.
   *
   * @param context - Object specifying the environment that the logger should interact with, e.g. the formatter
   * @param component - The name of the component or scope.
   * @internal
   */
  constructor(
    readonly context: LogContext,

    /**
     * An arbitrary name to uniquely identify the component this logger belongs to.
     *
     * @example "das:node:http"
     */
    readonly component: string,
  ) {}

  clear() {
    this.context.output(
      {
        type: "clear",
        date: Date.now(),
      },
      this.context,
    )
  }

  debug(message: string, ...parameters: unknown[]) {
    if (this.context.enableChecker(this.component)) {
      this.context.output(
        {
          type: "debug",
          date: Date.now(),
          namespace: this.component,
          message,
          parameters,
          caller: this.context.captureCaller
            ? this.context.getCaller(1, new Error())
            : undefined,
        },
        this.context,
      )
    }
  }

  info(message: string, ...parameters: unknown[]) {
    this.context.output(
      {
        type: "info",
        date: Date.now(),
        namespace: this.component,
        message,
        parameters,
        caller: this.context.captureCaller
          ? this.context.getCaller(1, new Error())
          : undefined,
      },
      this.context,
    )
  }

  warn(message: string, ...parameters: unknown[]) {
    this.context.output(
      {
        type: "warn",
        date: Date.now(),
        namespace: this.component,
        message,
        parameters,
        caller: this.context.captureCaller
          ? this.context.getCaller(1, new Error())
          : undefined,
      },
      this.context,
    )
  }

  error(message: string, ...parameters: unknown[]) {
    this.context.output(
      {
        type: "error",
        date: Date.now(),
        namespace: this.component,
        message,
        parameters,
        caller: this.context.captureCaller
          ? this.context.getCaller(1, new Error())
          : undefined,
      },
      this.context,
    )
  }
}

export interface LoggerOptions {
  context?: LogContext
}

const defaultContext = getLogContext()

/**
 * Create a logger factory which creates loggers using a given logging context.
 *
 * @alpha
 */
export const createLogger = (
  component: string,
  { context = defaultContext }: LoggerOptions = {},
) => {
  assert(!component.includes(" "), "Component name must not contain spaces")

  return new LoggerImplementation(context, component)
}
