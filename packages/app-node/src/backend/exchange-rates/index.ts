import { createActor } from "@dassie/lib-reactive"

import { exchangeRateService } from "./services/exchange-rate"

export const startExchangeRates = () =>
  createActor((sig) => {
    sig.run(sig.use(exchangeRateService).effect)
  })
