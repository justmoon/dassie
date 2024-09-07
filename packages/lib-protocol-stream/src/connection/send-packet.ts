import { IlpType } from "@dassie/lib-protocol-ilp"
import { isFailure } from "@dassie/lib-type-utils"

import { generateRandomCondition } from "../crypto/functions"
import { getPacketExpiry } from "../packets/expiry"
import { type StreamFrame, streamPacketSchema } from "../packets/schema"
import { NO_REMOTE_ADDRESS_FAILURE } from "./failures/no-remote-address-failure"
import type { ConnectionState } from "./state"

interface GenerateProbePacketsOptions {
  readonly state: Pick<
    ConnectionState,
    "context" | "pskEnvironment" | "nextSequence" | "remoteAddress"
  >
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
