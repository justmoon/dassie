import { z } from "zod"

import { createActor } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../config/database-config"
import { exchange as logger } from "../logger/instances"
import { INTERNAL_PRECISION } from "./constants/internal-precision"
import { ExchangeRatesSignal } from "./signals/exchange-rates"

const exchangeRateSchema = z.object({
  data: z.object({
    currency: z.string(),
    rates: z.record(z.string()),
  }),
})

export interface CurrencyDescription {
  code: string
  scale: number
}

export const LoadExchangeRatesActor = () =>
  createActor(async (sig) => {
    const { exchangeRateUrl } = sig.readKeysAndTrack(DatabaseConfigStore, [
      "exchangeRateUrl",
    ])

    const rateRequest = await fetch(exchangeRateUrl)

    const rates = exchangeRateSchema.parse(await rateRequest.json())

    logger.debug?.("loaded rates", {
      baseCurrency: rates.data.currency,
      currencyCount: Object.keys(rates.data.rates).length,
    })

    const normalizedRates: Record<string, bigint> = {}

    for (const [currency, rate] of Object.entries(rates.data.rates)) {
      normalizedRates[currency] = BigInt(
        Math.round(Number(rate) * 10 ** INTERNAL_PRECISION),
      )
    }

    sig.reactor.use(ExchangeRatesSignal).write({
      baseCurrency: rates.data.currency,
      rates: normalizedRates,
    })
  })
