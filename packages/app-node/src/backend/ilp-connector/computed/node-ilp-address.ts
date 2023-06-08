import { createComputed } from "@dassie/lib-reactive"

import { environmentConfigSignal } from "../.."
import { nodeIdSignal } from "./node-id"

export const nodeIlpAddressSignal = () =>
  createComputed((sig) => {
    const { ilpAllocationScheme } = sig.get(environmentConfigSignal)

    const nodeId = sig.get(nodeIdSignal)

    return `${ilpAllocationScheme}.das.${nodeId}`
  })
