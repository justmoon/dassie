import { IlpType, timestampToInterledgerTime } from "@dassie/lib-protocol-ilp"
import { isFailure } from "@dassie/lib-type-utils"

import { generateRandomCondition } from "../crypto/functions"
import { streamPacketSchema } from "../packets/schema"
import { NO_REMOTE_ADDRESS_FAILURE } from "./failures/no-remote-address-failure"
import type { ConnectionState } from "./state"

const DEFAULT_PACKET_TIMEOUT = 30_000

interface GenerateProbePacketsOptions {
  readonly state: Pick<
    ConnectionState,
    "context" | "pskEnvironment" | "nextSequence" | "remoteAddress"
  >
  readonly amount: bigint
}

export async function sendPacket({
  state,
  amount,
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
    frames: [],
  })

  const streamPacketEncrypted = await state.pskEnvironment.encrypt(streamPacket)

  const result = await context.sendPacket({
    destination: remoteAddress,
    amount,
    expiresAt: timestampToInterledgerTime(
      context.getDateNow() + DEFAULT_PACKET_TIMEOUT,
    ),
    executionCondition: generateRandomCondition(context.crypto),
    data: streamPacketEncrypted,
  })

  const streamPacketDecrypted = await state.pskEnvironment.decrypt(
    result.data.data,
  )

  const responseStreamPacket = streamPacketSchema.parse(streamPacketDecrypted)

  if (isFailure(responseStreamPacket)) {
    return responseStreamPacket
  }

  return responseStreamPacket
}
