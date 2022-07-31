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

export type PeerMessage = Infer<typeof peerMessage>

export const peerSignedHello = sequence({
  nodeId: ia5String(),
  sequence: uint64Bigint(),
  url: ia5String(),
  neighbors: sequenceOf(
    sequence({
      nodeId: ia5String(),
      proof: octetString(HELLO_NEIGHBOR_PROOF_LENGTH),
    })
  ),
})

export const peerSignedLinkStateUpdate = sequence({
  nodeId: ia5String(),
  sequence: uint64Bigint(),
  neighbors: sequenceOf(
    sequence({
      nodeId: ia5String(),
    })
  ),
})

export const peerMessage = choice({
  hello: sequence({
    signed: octetString().containing(peerSignedHello),
    signature: octetString(),
  }).tag(0),
  linkStateUpdate: sequence({
    signed: octetString().containing(peerSignedLinkStateUpdate),
    signature: octetString(),
  }).tag(1),
})
