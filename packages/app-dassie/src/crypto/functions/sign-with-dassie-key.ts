import { signAsync } from "@noble/ed25519"

import type { Reactor } from "@dassie/lib-reactive"

import { NodePrivateKeySignal } from "../computed/node-private-key"

export const SignWithDassieKey = (reactor: Reactor) => {
  const nodePrivateKeySignal = reactor.use(NodePrivateKeySignal)

  function signWithDassieKey(data: Uint8Array) {
    return signAsync(data, nodePrivateKeySignal.read())
  }

  return signWithDassieKey
}
