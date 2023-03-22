import { createActor } from "@dassie/lib-reactive"

import { exchangeRateService } from "./services/exchange-rate"

export const startExchangeRates = () =>
  createActor(async (sig) => {
    await sig.run(exchangeRateService, undefined, { register: true })
  })
