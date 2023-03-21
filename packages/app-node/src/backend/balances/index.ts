import { createActor } from "@dassie/lib-reactive"

import { keepOverallBalance } from "./keep-overall-balance"

export const startBalances = () =>
  createActor((sig) => {
    sig.run(keepOverallBalance)
  })
