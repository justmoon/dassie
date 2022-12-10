import type { EffectContext } from "@dassie/lib-reactive"

import { keepOverallBalance } from "./keep-overall-balance"

export const startBalances = (sig: EffectContext) => {
  sig.run(keepOverallBalance)
}
