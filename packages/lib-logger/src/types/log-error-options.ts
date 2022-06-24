/**
 * Options for the `logError` function.
 *
 * @beta
 */
export default interface LogErrorOptions {
  /**
   * Ignore any stack frames after this function.
   */
  skipAfter?: string

  /**
   * Don't log this error if we are running in production.
   *
   * This is useful to avoid logging expected operational errors in production which may still be useful to capture in development.
   */
  ignoreInProduction?: boolean
}
