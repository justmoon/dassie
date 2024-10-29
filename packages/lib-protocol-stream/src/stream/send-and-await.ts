import { createDeferred, createScope } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import { NoExchangeRateFailure } from "../connection/failures/no-exchange-rate-failure"
import { NoRemoteAddressFailure } from "../connection/failures/no-remote-address-failure"
import { sendUntilDone } from "../connection/send-until-done"
import type { ConnectionState } from "../connection/state"
import { addSendAmount } from "./add-send-amount"
import {
  SEND_INCOMPLETE_FAILURE,
  SEND_TIMEOUT_FAILURE,
  SendFailure,
} from "./failures/send-failure"
import type { RemoteMoneyEvent, StreamState } from "./state"

const DEFAULT_TIMEOUT = 30_000

interface SendAndAwaitOptions {
  connectionState: ConnectionState
  state: StreamState
  amount: bigint
  timeout?: number | undefined
}

export function sendAndAwait({
  connectionState,
  state,
  amount,
  timeout = DEFAULT_TIMEOUT,
}: SendAndAwaitOptions): Promise<
  void | SendFailure | NoRemoteAddressFailure | NoExchangeRateFailure
> {
  const scope = createScope("stream-send")

  {
    const result = addSendAmount({ connectionState, state, amount })
    if (isFailure(result)) return Promise.resolve(result)
  }

  const deferred = createDeferred<void | SendFailure>()
  const targetAmount = state.sendMaximum

  const handleSendCompleted = (result: void | SendFailure = undefined) => {
    scope.dispose().catch((error: unknown) => {
      connectionState.context.logger.error("error disposing send scope", {
        error,
      })
    })
    connectionState.context.clock.clearTimeout(timeoutId)
    deferred.resolve(result)
  }

  const timeoutId = connectionState.context.clock.setTimeout(() => {
    handleSendCompleted(SEND_TIMEOUT_FAILURE)
  }, timeout)

  const sentListener = () => {
    if (state.sentAmount >= targetAmount) {
      handleSendCompleted()
    }
  }
  state.topics.moneySent.on(scope, sentListener)

  const remoteListener = (event: RemoteMoneyEvent) => {
    if (
      event.receiveMaximum - event.receivedAmount <
      connectionState.context.policy.deMinimisAmount
    ) {
      handleSendCompleted()
    }
  }
  state.topics.remoteMoney.on(scope, remoteListener)

  sendUntilDone(connectionState)
    .catch((error: unknown) => {
      connectionState.context.logger.error(
        "unexpected error returned by send loop",
        {
          error,
        },
      )
    })
    .finally(() => {
      handleSendCompleted(SEND_INCOMPLETE_FAILURE)
    })

  return deferred
}
