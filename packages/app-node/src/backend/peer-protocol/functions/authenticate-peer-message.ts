import { x25519 } from "@noble/curves/ed25519"

import type { Reactor } from "@dassie/lib-reactive"

import { compareUint8Arrays } from "../../utils/compare-typedarray"
import { X25519PrivateKey } from "../computed/x25519-private-key"
import type { PeerMessage } from "../peer-schema"
import { IncomingSessionKeysStore } from "../stores/incoming-session-keys"
import type { NodeTableEntry } from "../stores/node-table"
import { calculateMessageHmac } from "../utils/calculate-message-hmac"

export const AuthenticatePeerMessage = (reactor: Reactor) => {
  const incomingSessionKeysStore = reactor.use(IncomingSessionKeysStore)
  const x25519PrivateKey = reactor.use(X25519PrivateKey)

  return (
    message: PeerMessage,
    senderNode: NodeTableEntry | undefined,
  ): boolean => {
    if (message.authentication.type === "NONE") {
      return false
    }

    if (!senderNode?.linkState) {
      return false
    }

    if (senderNode.peerState.id !== "peered") {
      return false
    }

    const sessionPublicKeyBase64 = Buffer.from(
      message.authentication.value.sessionPublicKey,
    ).toString("base64")

    let sessionKeysEntry = incomingSessionKeysStore
      .read()
      .get(sessionPublicKeyBase64)
    const wasSessionKeyCached = !!sessionKeysEntry

    if (!sessionKeysEntry) {
      const sharedSecret = x25519.getSharedSecret(
        x25519PrivateKey.read(),
        message.authentication.value.sessionPublicKey,
      )

      sessionKeysEntry = {
        sharedSecret,
        createdAt: new Date(),
      }
    }

    const expectedMessageAuthenticationCode = calculateMessageHmac(
      message.content.bytes,
      sessionKeysEntry.sharedSecret,
    )

    const isValid = compareUint8Arrays(
      expectedMessageAuthenticationCode,
      message.authentication.value.messageAuthenticationCode,
    )

    if (isValid && !wasSessionKeyCached) {
      incomingSessionKeysStore.act.addKeyEntry(
        sessionPublicKeyBase64,
        sessionKeysEntry,
      )
    }

    return isValid
  }
}
