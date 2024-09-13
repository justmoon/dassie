import { isFailure } from "@dassie/lib-type-utils"

import type { Ratio } from "../math/ratio"
import {
  EXCHANGE_RATE_NEGATIVE_FAILURE,
  EXCHANGE_RATE_ZERO_FAILURE,
  type InvalidExchangeRateFailure,
} from "./failures/invalid-exchange-rate-failure"
import type { NoRemoteAddressFailure } from "./failures/no-remote-address-failure"
import type { ProbingUnsuccessfulFailure } from "./failures/probing-unsuccessful-failure"
import { measureExchangeRate } from "./measure-exchange-rate"
import type { ConnectionState } from "./state"

/**
 * Set the minimum exchange rate.
 *
 * If the actual exchange rate for a packet is lower than this, the recipient will
 * be instructed to reject the packet.
 *
 * @param state - Connection state that should be updated with the new exchange rate.
 * @param exchangeRate - Exchange rate as a ratio of destination units divided by source units.
 * @returns
 */
export function setExchangeRate(
  state: ConnectionState,
  exchangeRate: Ratio,
): void | InvalidExchangeRateFailure {
  if (exchangeRate[0] < 0n || exchangeRate[1] < 0n) {
    return EXCHANGE_RATE_NEGATIVE_FAILURE
  }

  if (exchangeRate[0] === 0n || exchangeRate[1] === 0n) {
    return EXCHANGE_RATE_ZERO_FAILURE
  }

  state.exchangeRate = exchangeRate
}

export function dangerouslyIgnoreExchangeRate(state: ConnectionState) {
  state.exchangeRate = [0n, 1n]
}

export async function dangerouslyMeasureExchangeRate(
  state: ConnectionState,
): Promise<
  | void
  | NoRemoteAddressFailure
  | ProbingUnsuccessfulFailure
  | InvalidExchangeRateFailure
> {
  const exchangeRate = await measureExchangeRate({ state })

  if (isFailure(exchangeRate)) {
    return exchangeRate
  }

  return setExchangeRate(state, exchangeRate)
}
