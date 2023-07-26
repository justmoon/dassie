import { createActor } from "@dassie/lib-reactive"

import { attachLogger } from "./attach-logger"

export const startLogger = () =>
  createActor((sig) => {
    sig.run(attachLogger)
  })
