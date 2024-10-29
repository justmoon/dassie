import { isFailure } from "@dassie/lib-type-utils"

import { closeStream } from "../stream/close"
import { sendAndAwait } from "../stream/send-and-await"
import { createStream } from "./create-stream"
import type { ConnectionState } from "./state"

interface MakePaymentOptions {
  state: ConnectionState
  sourceAmountLimit: bigint
}

export async function makePayment({
  state,
  sourceAmountLimit,
}: MakePaymentOptions) {
  const { streamId, streamState } = createStream({ state })

  const result = await sendAndAwait({
    connectionState: state,
    state: streamState,
    amount: sourceAmountLimit,
  })

  if (isFailure(result)) return result

  closeStream(streamId, state, streamState)

  return
}
