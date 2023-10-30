import { getLoggingContext } from "./common"

export const CONSOLE_NAMESPACE = "console"
export const captureConsole = () => {
  const methods = ["debug", "info", "warn", "error"] as const

  const context = getLoggingContext()

  for (const method of methods) {
    // eslint-disable-next-line no-console
    console[method] = (message: string, ...parameters: unknown[]) => {
      if (method === "debug" && !context.enableChecker(CONSOLE_NAMESPACE))
        return

      context.output(
        {
          type: method,
          date: Date.now(),
          namespace: CONSOLE_NAMESPACE,
          message,
          parameters,
          caller: context.captureCaller
            ? // eslint-disable-next-line unicorn/error-message
              context.getCaller(1, new Error())
            : undefined,
        },
        context,
      )
    }
  }

  console.clear = () => {
    context.output(
      {
        type: "clear",
        date: Date.now(),
      },
      context,
    )
  }
}
