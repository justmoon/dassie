import {
  type Infer,
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
  uppercaseLetters,
  utf8String,
  visibleString,
} from "@dassie/lib-oer"

import { dassiePeerNodeInfoId } from "../constants/object-identifiers"
import { NodeId } from "./types/node-id"
import { SettlementSchemeId } from "./types/settlement-scheme-id"

export type PeerMessage = Infer<typeof peerMessage>

export const settlementSchemeIdSchema = ia5String([2, 128])
  .from(lowercaseLetters + digits + "-")
  .refine((_value): _value is SettlementSchemeId => true)

export const nodeIdSchema = ia5String([2, 45])
  .from(lowercaseLetters + uppercaseLetters + digits + "-_")
  .refine((_value): _value is NodeId => true)

export const nodeInfoEntry = choice({
  neighbor: sequence({
    nodeId: nodeIdSchema,
  }).tag(0),
  settlementScheme: sequence({
    settlementSchemeId: settlementSchemeIdSchema,
  }).tag(1),
})

export const peerNodeInfo = sequence({
  type: objectIdentifier().constant(dassiePeerNodeInfoId),
  nodeId: nodeIdSchema,
  sequence: uint64Bigint(),
  url: visibleString(),
  alias: utf8String(),
  publicKey: octetString(),
  entries: sequenceOf(nodeInfoEntry),
})

export const signaturePackage = choice({
  ed25519: octetString(64).tag(0),
})

export const signedPeerNodeInfo = sequence({
  signed: octetString().containing(peerNodeInfo),
  signature: signaturePackage,
})

export const peerInterledgerPacket = sequence({
  requestId: uint32Number(),
  packet: octetString(),
})

export const nodeListResponse = sequenceOf(captured(signedPeerNodeInfo))

export const peerMessageContent = choice({
  peeringRequest: sequence({
    settlementSchemeId: settlementSchemeIdSchema,
    nodeInfo: octetString().containing(signedPeerNodeInfo),
  }).tag(0),
  linkStateUpdate: captured(signedPeerNodeInfo).tag(1),
  interledgerPacket: sequence({
    signed: octetString().containing(peerInterledgerPacket),
  }).tag(2),
  linkStateRequest: sequence({
    nodeId: nodeIdSchema,
  }).tag(3),
  settlement: sequence({
    settlementSchemeId: settlementSchemeIdSchema,
    amount: uint64Bigint(),
    proof: octetString(),
  }).tag(4),
  settlementSchemeModuleMessage: sequence({
    settlementSchemeId: settlementSchemeIdSchema,
    message: octetString(),
  }).tag(5),
  nodeListRequest: sequence({}).tag(6),
  registration: sequence({
    nodeInfo: captured(signedPeerNodeInfo),
  }).tag(7),
})

export const peerMessage = sequence({
  version: uint8Number(),
  sender: nodeIdSchema,
  authentication: choice({
    ["NONE"]: sequence({}).tag(0),
    ["ED25519_X25519_HMAC-SHA256"]: sequence({
      sessionPublicKey: octetString(32),
    }).tag(1),
  }),
  content: captured(peerMessageContent),
})
