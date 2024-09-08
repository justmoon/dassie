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
          await state.pskEnvironment.getFulfillment(streamPacketEncrypted),
        )
      : generateRandomCondition(context.crypto),
    data: streamPacketEncrypted,
  })

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
