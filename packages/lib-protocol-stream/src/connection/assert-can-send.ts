import {
  NO_EXCHANGE_RATE_FAILURE,
  type NoExchangeRateFailure,
} from "./failures/no-exchange-rate-failure"
import {
  NO_REMOTE_ADDRESS_FAILURE,
  type NoRemoteAddressFailure,
} from "./failures/no-remote-address-failure"
import type { ConnectionState } from "./state"

export function assertConnectionCanSendMoney(
  state: ConnectionState,
): void | NoRemoteAddressFailure | NoExchangeRateFailure {
  if (state.remoteAddress === undefined) {
    return NO_REMOTE_ADDRESS_FAILURE
  }

  if (state.exchangeRate === undefined) {
    state.context.logger.error(
      "cannot send because no exchange rate has been set, use setExchangeRate, dangerouslyIgnoreExchangeRate, or dangerouslyMeasureExchangeRate before sending money",
    )
    return NO_EXCHANGE_RATE_FAILURE
  }
}
