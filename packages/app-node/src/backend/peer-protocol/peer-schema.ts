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
  uint8Number,
  uint32Number,
  uint64Bigint,
  visibleString,
} from "@dassie/lib-oer"

import { dassiePeerNodeInfoId } from "../constants/object-identifiers"

export type PeerMessage = Infer<typeof peerMessage>

export const subnetIdSchema = ia5String([2, 128]).from(
  lowercaseLetters + digits + "-"
)

export const nodeIdSchema = ia5String([2, 12]).from(lowercaseLetters + digits)

export const nodeInfoEntry = choice({
  neighbor: sequence({
    nodeId: nodeIdSchema,
  }).tag(0),
})

export const peerNodeInfo = sequence({
  type: objectIdentifier().constant(dassiePeerNodeInfoId),
  subnetId: subnetIdSchema,
  nodeId: nodeIdSchema,
  sequence: uint64Bigint(),
  url: visibleString(),
  nodePublicKey: octetString(),
  entries: sequenceOf(nodeInfoEntry),
})

export const signedPeerNodeInfo = sequence({
  signed: octetString().containing(peerNodeInfo),
  signature: octetString(),
})

export const peerInterledgerPacket = sequence({
  requestId: uint32Number(),
  packet: octetString(),
})

export const peerMessageContent = choice({
  peeringRequest: sequence({
    nodeInfo: octetString().containing(signedPeerNodeInfo),
  }).tag(0),
  linkStateUpdate: captured(signedPeerNodeInfo).tag(1),
  interledgerPacket: sequence({
    signed: octetString().containing(peerInterledgerPacket),
  }).tag(2),
})

export const peerMessage = sequence({
  version: uint8Number(),
  subnetId: subnetIdSchema,
  sender: nodeIdSchema,
  authentication: choice({
    ["NONE"]: sequence({}).tag(0),
    ["ED25519_X25519_HMAC-SHA256"]: sequence({
      sessionPublicKey: octetString(32),
    }).tag(1),
  }),
  content: captured(peerMessageContent),
})
