import { edwardsToMontgomeryPriv } from "@noble/curves/ed25519"

import { Reactor, createComputed } from "@dassie/lib-reactive"

import { NodePrivateKeySignal } from "../../crypto/computed/node-private-key"

export const X25519PrivateKey = (reactor: Reactor) => {
  const nodePrivateKeySignal = reactor.use(NodePrivateKeySignal)
  return createComputed(reactor.lifecycle, (sig) => {
    const dassieKey = sig.get(nodePrivateKeySignal)
    return edwardsToMontgomeryPriv(dassieKey)
  })
}
