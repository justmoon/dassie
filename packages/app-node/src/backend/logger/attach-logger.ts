import { captureConsole, createCliFormatter } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

export const attachLogger = () =>
  createActor(() => {
    captureConsole({ formatter: createCliFormatter() })
  })
