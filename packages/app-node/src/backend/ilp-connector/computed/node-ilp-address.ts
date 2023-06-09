import { createComputed } from "@dassie/lib-reactive"

import { ilpAllocationSchemeSignal } from "../../config/computed/ilp-allocation-scheme"
import { nodeIdSignal } from "./node-id"

export const nodeIlpAddressSignal = () =>
  createComputed((sig) => {
    const ilpAllocationScheme = sig.get(ilpAllocationSchemeSignal)

    const nodeId = sig.get(nodeIdSignal)

    return `${ilpAllocationScheme}.das.${nodeId}`
  })
