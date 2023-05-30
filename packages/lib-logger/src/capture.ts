import { LogEventFormatter } from "./types/formatter"

export interface CaptureParameters {
  formatter: LogEventFormatter
  outputFunction?: (line: string) => void
}

export const captureConsole = ({ formatter }: CaptureParameters) => {
  const methods = ["debug", "info", "warn", "error"] as const

  for (const method of methods) {
    console[method] = (message: string, ...parameters: unknown[]) => {
      formatter({
        type: method,
        date: new Date(),
        message,
        parameters,
      })
    }
  }

  console.clear = () => {
    formatter({
      type: "clear",
      date: new Date(),
    })
  }
}
