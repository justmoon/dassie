import axios from "axios"
import { z } from "zod"

import { createLogger } from "@dassie/lib-logger"
import { createService } from "@dassie/lib-reactive"

import { configSignal } from "../../config"

const logger = createLogger("das:node:exchange-rate-service")

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

export const exchangeRateService = () =>
  createService(async (sig) => {
    const { exchangeRateUrl, internalAmountPrecision } = sig.getKeys(
      configSignal,
      ["exchangeRateUrl", "internalAmountPrecision"]
    )

    const rateRequest = await axios(exchangeRateUrl)

    const rates = exchangeRateSchema.parse(rateRequest.data)

    logger.debug("loaded rates", {
      baseCurrency: rates.data.currency,
      currencyCount: Object.keys(rates.data.rates).length,
    })

    const exchangeRateConverter = {
      /**
       * Calculate the amount of the base currency that is equivalent to the source amount.
       * @param from - Currency description containing the code and scale of the source currency
       * @param sourceAmount - Amount, expressed in the source currency's scale
       */
      convertToBaseFrom(from: CurrencyDescription, sourceAmount: bigint) {
        const rate = rates.data.rates[from.code]

        if (!rate) {
          throw new Error(`No rate available for currency ${from.code}`)
        }

        const rateBigInt = BigInt(
          Math.round(Number(rate) * 10 ** internalAmountPrecision)
        )

        const baseAmount =
          (sourceAmount * rateBigInt) / BigInt(10 ** from.scale)

        return baseAmount
      },
      convertFromBaseTo(to: CurrencyDescription, sourceAmount: bigint) {
        const rate = rates.data.rates[to.code]

        if (!rate) {
          throw new Error(`No rate available for currency ${to.code}`)
        }

        const rateBigInt = BigInt(
          Math.round(Number(rate) * 10 ** internalAmountPrecision)
        )

        const targetAmount =
          (sourceAmount * BigInt(10 ** to.scale)) / rateBigInt

        return targetAmount
      },
      convert(
        from: CurrencyDescription,
        to: CurrencyDescription,
        sourceAmount: bigint
      ) {
        const baseAmount = exchangeRateConverter.convertToBaseFrom(
          from,
          sourceAmount
        )
        const targetAmount = exchangeRateConverter.convertFromBaseTo(
          to,
          baseAmount
        )

        return targetAmount
      },
    }

    return exchangeRateConverter
  })
