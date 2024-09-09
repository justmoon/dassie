import {
  IlpErrorCode,
  IlpType,
  amountTooLargeDataSchema,
} from "@dassie/lib-protocol-ilp"
import { isFailure } from "@dassie/lib-type-utils"

import { generateRandomCondition } from "../crypto/functions"
import { multiplyByRatio } from "../math/ratio"
import { getPacketExpiry } from "../packets/expiry"
import { type StreamFrame, streamPacketSchema } from "../packets/schema"
import { NO_REMOTE_ADDRESS_FAILURE } from "./failures/no-remote-address-failure"
import type { ConnectionState } from "./state"

interface GenerateProbePacketsOptions {
  readonly state: ConnectionState
  readonly amount: bigint
  readonly fulfillable?: boolean | undefined
  readonly frames?: StreamFrame[] | undefined
}

export async function sendPacket({
  state,
  amount,
  fulfillable = false,
  frames = [],
}: GenerateProbePacketsOptions) {
  const { context, remoteAddress } = state

  if (!remoteAddress) {
    context.logger.warn("cannot send packet without a remote address")
    return NO_REMOTE_ADDRESS_FAILURE
  }

  // TODO: Handle retries and errors
  const streamPacket = streamPacketSchema.serializeOrThrow({
    packetType: IlpType.Prepare,
    sequence: BigInt(state.nextSequence++),
    amount,
    frames,
  })

  const streamPacketEncrypted = await state.pskEnvironment.encrypt(streamPacket)

  const result = await context.endpoint.sendPacket({
    destination: remoteAddress,
    amount,
    expiresAt: getPacketExpiry(context),
    executionCondition:
      fulfillable ?
        await context.crypto.hash(
          "sha256",
          await state.pskEnvironment.getFulfillment(streamPacketEncrypted),
        )
      : generateRandomCondition(context.crypto),
    data: streamPacketEncrypted,
  })

  // Handle F08 errors indicating that the packets we're sending are too large
  if (
    result.type === IlpType.Reject &&
    result.data.code === IlpErrorCode.F08_AMOUNT_TOO_LARGE
  ) {
    const amountTooLargeData = amountTooLargeDataSchema.parse(result.data.data)

    if (isFailure(amountTooLargeData)) {
      context.logger.warn("received invalid F08 data, ignoring")
    } else if (
      amountTooLargeData.value.maximumAmount >=
      amountTooLargeData.value.receivedAmount
    ) {
      context.logger.warn(
        "received F08 with maximum amount >= received amount, ignoring",
      )
    } else {
      const newMaximum = multiplyByRatio(amount, [
        amountTooLargeData.value.maximumAmount,
        amountTooLargeData.value.receivedAmount,
      ])

      context.logger.debug?.("received F08, adjusting maximum packet amount", {
        receivedAmount: amountTooLargeData.value.receivedAmount,
        maximumAmount: amountTooLargeData.value.maximumAmount,
        oldMax: state.maximumPacketAmount,
        newMax: newMaximum,
      })

      state.maximumPacketAmount = newMaximum
    }
  }

  // Increase concurrency whenever we successfully send a packet
  if (result.type === IlpType.Fulfill) {
    // Additively increase concurrency as part of AIMD congestion control.
    state.concurrency = Math.min(
      state.concurrency + context.policy.concurrencyIncreaseIncrement,
      context.policy.maximumConcurrency,
    )
  }

  // Decreate concurrency when packets are being rejected due to a temporary error
  // such as overloading.
  if (result.type === IlpType.Reject && result.data.code.startsWith("T")) {
    // Multiplicatively decrease concurrency as part of AIMD congestion control.
    //
    // Please note that this value is intentionally not rounded.
    state.concurrency = Math.max(
      state.concurrency * context.policy.concurrencyDecreaseFactor,
      context.policy.minimumConcurrency,
    )
  }

  const streamPacketDecrypted = await state.pskEnvironment.decrypt(
    result.data.data,
  )

  if (isFailure(streamPacketDecrypted)) {
    return { ilp: result, stream: streamPacketDecrypted }
  }

  const responseStreamPacket = streamPacketSchema.parse(streamPacketDecrypted)

  if (isFailure(responseStreamPacket)) {
    return { ilp: result, stream: responseStreamPacket }
  }

  return { ilp: result, stream: responseStreamPacket }
}
