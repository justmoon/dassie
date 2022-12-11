import type { EffectContext } from "@dassie/lib-reactive"

import { exchangeRateService } from "./services/exchange-rate"

export const startExchangeRates = (sig: EffectContext) => {
  sig.run(sig.use(exchangeRateService).effect)
}
