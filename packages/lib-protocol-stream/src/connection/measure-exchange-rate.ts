import { isFailure } from "@dassie/lib-type-utils"

import type { Ratio } from "../math/ratio"
import { NO_REMOTE_ADDRESS_FAILURE } from "./failures/no-remote-address-failure"
import { PROBING_UNSUCCESSFUL_FAILURE } from "./failures/probing-unsuccessful-failure"
import { sendPacket } from "./send-packet"
import type { ConnectionState } from "./state"

/**
 * The amounts to probe with.
 *
 * @remarks
 *
 * Originally, in `ilp-protocol-stream`, probing was done with a single packet
 * of 1e3 units. This was
 * [changed](https://github.com/interledgerjs/interledgerjs/commit/82c6fee57ed83ae487a354c32d1438e89ee0a4f0) to `[1, 1e3, 1e6, 1e9]` and
 * [later](https://github.com/interledgerjs/interledgerjs/commit/de6660ad7ac53fb91449d330433f08b0d8ace2dd) to `[1, 1e3, 1e6, 1e9, 1e12]`.
 */
const PROBE_PACKET_AMOUNTS = [1, 1e3, 1e6, 1e9, 1e12].map(BigInt)

interface GenerateProbePacketsOptions {
  readonly state: Pick<
    ConnectionState,
    "context" | "pskEnvironment" | "nextSequence" | "remoteAddress"
  >
}

export async function measureExchangeRate({
  state,
}: GenerateProbePacketsOptions) {
  const { context, remoteAddress } = state

  if (!remoteAddress) {
    context.logger.warn("cannot probe connection without a remote address")
    return NO_REMOTE_ADDRESS_FAILURE
  }

  // TODO: Handle retries
  const results = await Promise.all(
    PROBE_PACKET_AMOUNTS.map(async (amount) => {
      const response = await sendPacket({ state, amount })

      if (isFailure(response)) {
        return response
      }

      if (isFailure(response.stream)) {
        return response.stream
      }

      const exchangeRate: Ratio = [response.stream.value.amount, amount]
      const digits = Math.log10(Number(amount))

      return { exchangeRate, digits }
    }),
  )

  let exchangeRate: Ratio = [0n, 0n]
  let digits = 0

  for (const result of results) {
    if (isFailure(result)) continue

    if (result.digits > digits) {
      exchangeRate = result.exchangeRate
      digits = result.digits
    }
  }

  if (digits === 0) {
    context.logger.warn("no probe packets were successful")
    return PROBING_UNSUCCESSFUL_FAILURE
  }

  // Strip excess trailing zeros
  while (exchangeRate[0] % 10n === 0n && exchangeRate[1] % 10n === 0n) {
    exchangeRate = [exchangeRate[0] / 10n, exchangeRate[1] / 10n]
  }

  context.logger.info("exchange rate measured successfully", {
    exchangeRate,
    approximation: Number(exchangeRate[0]) / Number(exchangeRate[1]),
    digits,
  })

  return exchangeRate
}