import { type Reactor, createComputed } from "@dassie/lib-reactive"

import { IlpAllocationSchemeSignal } from "../../config/computed/ilp-allocation-scheme"
import type { DassieIlpAddress } from "../types/ilp-address"
import { NodeIdSignal } from "./node-id"

export const NodeIlpAddressSignal = (reactor: Reactor) =>
  createComputed(reactor, (sig): DassieIlpAddress => {
    const ilpAllocationScheme = sig.readAndTrack(IlpAllocationSchemeSignal)

    const nodeId = sig.readAndTrack(reactor.use(NodeIdSignal))

    return `${ilpAllocationScheme}.das.${nodeId}`
  })
