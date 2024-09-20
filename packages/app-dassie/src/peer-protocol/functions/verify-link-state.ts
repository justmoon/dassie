import { verifyAsync } from "@noble/ed25519"

import { assert } from "@dassie/lib-logger"
import type { Infer } from "@dassie/lib-oer"

import { calculateNodeId } from "../../ilp-connector/utils/calculate-node-id"
import { peerProtocol as logger } from "../../logger/instances"
import InvalidLinkStateFailure from "../failures/invalid-link-state"
import { peerNodeInfo, signaturePackage } from "../peer-schema"

export interface VerifyLinkStateParameters {
  linkStateSignedBytes: Uint8Array
  linkState: Omit<Infer<typeof peerNodeInfo>, "type">
  signature: Infer<typeof signaturePackage>
}

const INVALID_SIGNATURE = new InvalidLinkStateFailure("invalid signature")

const INVALID_NODE_ID = new InvalidLinkStateFailure("invalid node ID")

export async function verifyLinkState({
  linkStateSignedBytes: linkStateBytes,
  linkState,
  signature,
}: VerifyLinkStateParameters): Promise<void | InvalidLinkStateFailure> {
  const publicKey = linkState.publicKey

  assert(
    logger,
    // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition
    signature.type === "ed25519",
    "only ed25519 signatures are supported at the moment",
  )

  const expectedNodeId = calculateNodeId(publicKey)

  if (expectedNodeId !== linkState.nodeId) {
    return INVALID_NODE_ID
  }

  const valid = await verifyAsync(signature.value, linkStateBytes, publicKey)

  if (!valid) return INVALID_SIGNATURE

  // All valid
  return
}
