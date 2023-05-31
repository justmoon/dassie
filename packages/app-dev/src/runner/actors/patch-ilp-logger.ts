import { createRequire } from "node:module"

import { defaultTheme, selectBySeed } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

export const patchIlpLogger = () =>
  createActor(() => {
    const ownRequire = createRequire(import.meta.url)
    const streamRequire = createRequire(
      ownRequire.resolve("ilp-protocol-stream")
    )
    const loggerRequire = createRequire(streamRequire.resolve("ilp-logger"))
    const debug = loggerRequire("debug") as unknown
    ;(debug as { log: typeof console.debug }).log = (
      message: string,
      ...parameters: unknown[]
    ) => {
      const firstSpace = message.indexOf(" ")
      if (firstSpace !== -1) {
        const namespace = message.slice(0, firstSpace)
        message = `${selectBySeed(
          defaultTheme.colors,
          namespace
        )(namespace)} ${message.slice(firstSpace + 1)}`
      }
      console.debug(message, ...parameters)
    }
  })
