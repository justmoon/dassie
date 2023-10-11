import { createActor } from "@dassie/lib-reactive"

import { ExchangeRateServiceActor } from "./services/exchange-rate"

export const ExchangeRatesActor = () =>
  createActor(async (sig) => {
    await sig.run(ExchangeRateServiceActor)
  })
