import { createActor } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { primarySubnetSignal } from "../subnets/signals/primary-subnet"
import { nodeIdSignal } from "./computed/node-id"
import { primaryIlpAddressSignal } from "./signals/primary-ilp-address"

export const managePrimaryIlpAddress = () =>
  createActor((sig) => {
    const nodeId = sig.get(nodeIdSignal)
    const { ilpAllocationScheme } = sig.getKeys(configSignal, [
      "ilpAllocationScheme",
    ])
    const defaultSubnet = sig.get(primarySubnetSignal)

    sig
      .use(primaryIlpAddressSignal)
      .write(
        defaultSubnet
          ? `${ilpAllocationScheme}.das.${defaultSubnet}.${nodeId}`
          : undefined
      )
  })
