import { edwardsToMontgomeryPriv } from "@noble/curves/ed25519"

import { type Reactor, createComputed } from "@dassie/lib-reactive"

import { NodePrivateKeySignal } from "../../crypto/computed/node-private-key"

export const X25519PrivateKey = (reactor: Reactor) => {
  const nodePrivateKeySignal = reactor.use(NodePrivateKeySignal)
  return createComputed(reactor, (sig) => {
    const dassieKey = sig.readAndTrack(nodePrivateKeySignal)
    return edwardsToMontgomeryPriv(dassieKey)
  })
}
