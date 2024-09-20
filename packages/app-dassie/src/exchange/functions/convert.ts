import type { Reactor } from "@dassie/lib-reactive"

import { INTERNAL_PRECISION } from "../constants/internal-precision"
import type { CurrencyDescription } from "../load-exchange-rates"
import { ExchangeRatesSignal } from "../signals/exchange-rates"

export const ConvertCurrencyAmounts = (reactor: Reactor) => {
  const exchangeRatesSignal = reactor.use(ExchangeRatesSignal)

  /**
   * Calculate the amount of the base currency that is equivalent to the source amount.
   * @param from - Currency description containing the code and scale of the source currency
   * @param sourceAmount - Amount, expressed in the source currency's scale
   */
  function toBaseFrom(from: CurrencyDescription, sourceAmount: bigint) {
    const { rates } = exchangeRatesSignal.read()

    const rate = rates[from.code]

    if (rate === undefined) {
      throw new Error(`No rate available for currency ${from.code}`)
    }

    const baseAmount =
      (sourceAmount * BigInt(10 ** (INTERNAL_PRECISION * 2 - from.scale))) /
      rate

    return baseAmount
  }

  function fromBaseTo(to: CurrencyDescription, sourceAmount: bigint) {
    const { rates } = exchangeRatesSignal.read()

    const rate = rates[to.code]

    if (rate === undefined) {
      throw new Error(`No rate available for currency ${to.code}`)
    }

    const targetAmount =
      (sourceAmount * rate) / BigInt(10 ** (INTERNAL_PRECISION * 2 - to.scale))

    return targetAmount
  }

  function convertCurrencyAmounts(
    from: CurrencyDescription,
    to: CurrencyDescription,
    sourceAmount: bigint,
  ) {
    const baseAmount = toBaseFrom(from, sourceAmount)
    const targetAmount = fromBaseTo(to, baseAmount)

    return targetAmount
  }

  convertCurrencyAmounts.toBaseFrom = toBaseFrom
  convertCurrencyAmounts.fromBaseTo = fromBaseTo

  return convertCurrencyAmounts
}
