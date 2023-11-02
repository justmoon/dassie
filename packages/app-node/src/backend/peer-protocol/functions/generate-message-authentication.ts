import { edwardsToMontgomeryPub, x25519 } from "@noble/curves/ed25519"

import { createHmac } from "node:crypto"

import { Reactor } from "@dassie/lib-reactive"

import { bufferToUint8Array } from "../../utils/buffer-to-typedarray"
import { PeerMessageType } from "../actors/handle-peer-message"
import { ALLOW_ANONYMOUS_USAGE } from "../constants/anonymous-messages"
import { PeerMessage } from "../peer-schema"
import {
  OutgoingSessionKeysStore,
  SESSION_KEY_EXPIRY,
} from "../stores/outgoing-session-keys"
import { NodeId } from "../types/node-id"

const NO_AUTHENTICATION = {
  type: "NONE",
  value: undefined,
} as const

export const GenerateMessageAuthentication = (reactor: Reactor) => {
  const outgoingSessionKeysSignal = reactor.use(OutgoingSessionKeysStore)
  return (
    serializedMessage: Uint8Array,
    messageType: PeerMessageType,
    destination: NodeId,
    destinationPublicKey: Uint8Array,
  ): PeerMessage["authentication"] => {
    if (ALLOW_ANONYMOUS_USAGE.includes(messageType)) {
      return NO_AUTHENTICATION
    }

    let sessionKeysEntry = outgoingSessionKeysSignal.read().get(destination)
    if (
      !sessionKeysEntry ||
      Number(sessionKeysEntry.createdAt) + SESSION_KEY_EXPIRY < Date.now()
    ) {
      const sessionPrivateKey = x25519.utils.randomPrivateKey()
      sessionKeysEntry = {
        sessionPublicKey: x25519.getPublicKey(sessionPrivateKey),
        sharedSecret: x25519.getSharedSecret(
          sessionPrivateKey,
          edwardsToMontgomeryPub(destinationPublicKey),
        ),
        createdAt: new Date(),
      }
      outgoingSessionKeysSignal.addKeyEntry(destination, sessionKeysEntry)
    }

    const hmac = createHmac("sha256", sessionKeysEntry.sharedSecret)

    hmac.update(serializedMessage)

    const messageAuthenticationCode = bufferToUint8Array(hmac.digest())

    return {
      type: "ED25519_X25519_HMAC-SHA256" as const,
      value: {
        sessionPublicKey: sessionKeysEntry.sessionPublicKey,
        messageAuthenticationCode,
      },
    }
  }
}
