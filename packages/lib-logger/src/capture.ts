import { createLogger } from "./logger"

export const captureConsole = () => {
  const methods = ["debug", "info", "warn", "error"] as const

  // Creating a logger here has the side effect of making sure that the global
  // logging context is initialized which keeps a copy of the real console.
  const logger = createLogger("console")

  for (const method of methods) {
    // eslint-disable-next-line no-console
    console[method] = (message: string, ...parameters: unknown[]) => {
      logger[method](message, parameters)
    }
  }

  console.clear = () => {
    logger.clear()
  }
}
