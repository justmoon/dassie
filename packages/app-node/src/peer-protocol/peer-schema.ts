import {
  Infer,
  captured,
  choice,
  digits,
  ia5String,
  lowercaseLetters,
  objectIdentifier,
  octetString,
  sequence,
  sequenceOf,
  uint64Bigint,
  visibleString,
} from "@dassie/lib-oer"

import {
  dassiePeerNodeInfoId,
  dassiePeerSignedHelloId,
} from "../constants/object-identifiers"

export type PeerMessage = Infer<typeof peerMessage>

export const nodeIdSchema = ia5String().from(lowercaseLetters + digits)

export const nodeInfoEntry = choice({
  neighbor: sequence({
    nodeId: nodeIdSchema,
  }).tag(0),
})

export const peerNodeInfo = sequence({
  type: objectIdentifier().constant(dassiePeerNodeInfoId),
  nodeId: nodeIdSchema,
  sequence: uint64Bigint(),
  url: visibleString(),
  publicKey: octetString(),
  entries: sequenceOf(nodeInfoEntry),
})

export const signedPeerNodeInfo = sequence({
  signed: octetString().containing(peerNodeInfo),
  signature: octetString(),
})

export const peerHello = sequence({
  type: objectIdentifier().constant(dassiePeerSignedHelloId),
  nodeInfo: octetString().containing(signedPeerNodeInfo),
})

export const peerMessage = choice({
  hello: sequence({
    signed: octetString().containing(peerHello),
    signature: octetString(),
  }).tag(0),
  linkStateUpdate: captured(signedPeerNodeInfo).tag(1),
})
