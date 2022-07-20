/**
 * Options for the `log` function.
 *
 * @beta
 */
export interface LogLineOptions {
  /**
   * Don't log this message if we are running in production.
   *
   * This is useful to avoid logging expected operational errors in production which may still be useful to capture in development.
   */
  ignoreInProduction?: boolean

  /**
   * Ignore any stack frames after this function.
   */
  skipAfter?: string
}
