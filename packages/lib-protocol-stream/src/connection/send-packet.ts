import type { ParseFailure } from "@dassie/lib-oer"
import {
  IlpErrorCode,
  type IlpResponsePacket,
  IlpType,
  amountTooLargeDataSchema,
} from "@dassie/lib-protocol-ilp"
import type { DecryptionFailure } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import { generateRandomCondition } from "../crypto/functions"
import { multiplyByRatio } from "../math/ratio"
import { getPacketExpiry } from "../packets/expiry"
import {
  type StreamFrame,
  type StreamPacket,
  streamPacketSchema,
} from "../packets/schema"
import {
  NO_REMOTE_ADDRESS_FAILURE,
  type NoRemoteAddressFailure,
} from "./failures/no-remote-address-failure"
import { handleControlFrame } from "./handle-control-frames"
import type { ConnectionState } from "./state"

interface SendPacketOptions {
  readonly state: ConnectionState
  readonly sourceAmount: bigint
  readonly destinationAmount: bigint
  readonly fulfillable?: boolean | undefined
  readonly frames?: StreamFrame[] | undefined
}

export async function sendPacket({
  state,
  sourceAmount: sourceAmount,
  destinationAmount: destinationAmount,
  fulfillable = false,
  frames = [],
}: SendPacketOptions): Promise<
  | NoRemoteAddressFailure
  | {
      ilp: IlpResponsePacket
      stream: StreamPacket | DecryptionFailure | ParseFailure
    }
> {
  const { context, remoteAddress } = state

  if (!remoteAddress) {
    context.logger.warn("cannot send packet without a remote address")
    return NO_REMOTE_ADDRESS_FAILURE
  }

  // TODO: Handle retries and errors
  const streamPacket = streamPacketSchema.serializeOrThrow({
    packetType: IlpType.Prepare,
    sequence: BigInt(state.nextSequence++),
    amount: destinationAmount,
    frames,
  })

  const streamPacketEncrypted = await state.pskEnvironment.encrypt(streamPacket)

  const result = await context.endpoint.sendPacket({
    destination: remoteAddress,
    amount: sourceAmount,
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
      const newMaximum = multiplyByRatio(sourceAmount, [
        amountTooLargeData.value.maximumAmount,
        amountTooLargeData.value.receivedAmount,
      ])

      context.logger.debug?.("received F08, adjusting maximum packet amount", {
        receivedAmount: amountTooLargeData.value.receivedAmount,
        maximumAmount: amountTooLargeData.value.maximumAmount,
        oldMax: state.maxPacketAmount,
        newMax: newMaximum,
      })

      state.maxPacketAmount = newMaximum
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

  // Decreate concurrency when packets are being rejected
  if (result.type === IlpType.Reject && fulfillable) {
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

  const streamPacketParseResult = streamPacketSchema.parse(
    streamPacketDecrypted,
  )

  if (isFailure(streamPacketParseResult)) {
    return { ilp: result, stream: streamPacketParseResult }
  }

  const responseStreamPacket = streamPacketParseResult.value

  for (const frame of responseStreamPacket.frames) {
    handleControlFrame({ state, frame })
  }

  return { ilp: result, stream: responseStreamPacket }
}
