import { createRequire } from "node:module"

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
      ...parameters: unknown[]
    ) => console.debug(...parameters)
  })
