import { createComputed } from "@dassie/lib-reactive"

import { configSignal } from "../.."
import { nodeIdSignal } from "./node-id"

export const nodeIlpAddressSignal = () =>
  createComputed((sig) => {
    const { ilpAllocationScheme } = sig.get(configSignal)

    const nodeId = sig.get(nodeIdSignal)

    return `${ilpAllocationScheme}.das.${nodeId}`
  })
