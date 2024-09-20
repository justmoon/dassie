import { createActor } from "@dassie/lib-reactive"

import { LoadExchangeRatesActor } from "./load-exchange-rates"

export const ExchangeRatesActor = () =>
  createActor(async (sig) => {
    await sig.run(LoadExchangeRatesActor)
  })
