import { createHash } from "node:crypto"

import { Reactor, createComputed } from "@dassie/lib-reactive"

import { SerializedNodeListSignal } from "./serialized-node-list"

export const NodeListHashSignal = (reactor: Reactor) =>
  createComputed(reactor, (sig) => {
    const nodeList = sig.get(reactor.use(SerializedNodeListSignal))

    const hash = createHash("sha256")

    hash.update(nodeList)

    return hash.digest()
  })
