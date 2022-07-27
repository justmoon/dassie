import type { MarkOptional } from "ts-essentials"

import {
  Infer,
  choice,
  ia5String,
  octetString,
  sequence,
  sequenceOf,
  uint64Bigint,
} from "@xen-ilp/lib-oer"

const HELLO_NEIGHBOR_PROOF_LENGTH = 32

export interface XenEnvelope
  extends Omit<Infer<typeof xenEnvelope>, "message"> {
  message: XenMessage
}

export type XenMessage = Infer<typeof xenMessage>

export const xenEnvelope = sequence({
  nodeId: ia5String(),
  message: octetString(),
  signature: octetString(64),
})

export const xenMessage = choice({
  hello: sequence({
    sequence: uint64Bigint(),
    url: ia5String(),
    neighbors: sequenceOf(
      sequence({
        nodeId: ia5String(),
        proof: octetString(HELLO_NEIGHBOR_PROOF_LENGTH),
      })
    ),
  }),
})

export const parseEnvelope = (envelope: Uint8Array): XenEnvelope => {
  const envelopeParseResult = xenEnvelope.parse(envelope)

  if (!envelopeParseResult.success) {
    throw new Error("Failed to parse envelope", {
      cause: envelopeParseResult.failure,
    })
  }

  const messageParseResult = xenMessage.parse(envelopeParseResult.value.message)

  if (!messageParseResult.success) {
    throw new Error("Failed to parse message", {
      cause: messageParseResult.failure,
    })
  }

  return {
    ...envelopeParseResult.value,
    message: messageParseResult.value,
  }
}

export type XenEnvelopeWithOptionalSignature = MarkOptional<
  XenEnvelope,
  "signature"
>

export function encodeMessage(message: XenEnvelope): Uint8Array
export function encodeMessage(
  message: XenEnvelopeWithOptionalSignature,
  sign: (content: Uint8Array) => Uint8Array
): Uint8Array
export function encodeMessage(
  envelope: XenEnvelopeWithOptionalSignature,
  sign?: (content: Uint8Array) => Uint8Array
): Uint8Array {
  const messageSerializeResult = xenMessage.serialize(envelope.message)

  if (!messageSerializeResult.success) {
    throw new Error("Failed to serialize message", {
      cause: messageSerializeResult.failure,
    })
  }

  const envelopeSerializeResult = xenEnvelope.serialize({
    ...envelope,
    message: messageSerializeResult.value,
    signature: envelope.signature ?? sign!(messageSerializeResult.value),
  })

  if (!envelopeSerializeResult.success) {
    throw new Error("Failed to serialize envelope", {
      cause: envelopeSerializeResult.failure,
    })
  }

  return envelopeSerializeResult.value
}
