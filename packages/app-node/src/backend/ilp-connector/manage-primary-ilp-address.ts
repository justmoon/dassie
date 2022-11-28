import type { EffectContext } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { primarySubnetSignal } from "../subnets/signals/primary-subnet"
import { primaryIlpAddressSignal } from "./signals/primary-ilp-address"

export const managePrimaryIlpAddress = (sig: EffectContext) => {
  const { ilpAllocationScheme, nodeId } = sig.getKeys(configSignal, [
    "ilpAllocationScheme",
    "nodeId",
  ])
  const defaultSubnet = sig.get(primarySubnetSignal)

  sig
    .use(primaryIlpAddressSignal)
    .write(
      defaultSubnet
        ? `${ilpAllocationScheme}.das.${defaultSubnet}.${nodeId}`
        : undefined
    )
}
