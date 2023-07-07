import { createActor } from "@dassie/lib-reactive"

import { notifySystemd } from "./notify"

export const supportSystemd = () =>
  createActor((sig) => {
    sig.run(notifySystemd)
  })
