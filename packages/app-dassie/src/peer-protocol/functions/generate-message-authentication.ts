import { edwardsToMontgomeryPub, x25519 } from "@noble/curves/ed25519"

import type { Reactor } from "@dassie/lib-reactive"

import { ALLOW_ANONYMOUS_USAGE } from "../constants/anonymous-messages"
import type { PeerMessage } from "../peer-schema"
import {
  OutgoingSessionKeysStore,
  SESSION_KEY_EXPIRY,
} from "../stores/outgoing-session-keys"
import type { NodeId } from "../types/node-id"
import { calculateMessageHmac } from "../utils/calculate-message-hmac"
import type { PeerMessageType } from "./handle-peer-message"

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
      outgoingSessionKeysSignal.act.addKeyEntry(destination, sessionKeysEntry)
    }

    const messageAuthenticationCode = calculateMessageHmac(
      serializedMessage,
      sessionKeysEntry.sharedSecret,
    )

    return {
      type: "ED25519_X25519_HMAC-SHA256" as const,
      value: {
        sessionPublicKey: sessionKeysEntry.sessionPublicKey,
        messageAuthenticationCode,
      },
    }
  }
}
