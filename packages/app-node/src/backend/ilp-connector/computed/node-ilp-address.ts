import { Reactor, createComputed } from "@dassie/lib-reactive"

import { IlpAllocationSchemeSignal } from "../../config/computed/ilp-allocation-scheme"
import { NodeIdSignal } from "./node-id"

export const NodeIlpAddressSignal = (reactor: Reactor) =>
  createComputed(reactor, (sig) => {
    const ilpAllocationScheme = sig.get(reactor.use(IlpAllocationSchemeSignal))

    const nodeId = sig.get(reactor.use(NodeIdSignal))

    return `${ilpAllocationScheme}.das.${nodeId}`
  })
