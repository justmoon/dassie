import type { EffectContext } from "@dassie/lib-reactive"

import type { SubnetModuleInstance } from "../subnets/types/subnet-module"
import { calculateRoutes } from "./calculate-routes"
import { handlePeerMessages } from "./handle-peer-messages"
import { maintainOwnNodeTableEntry } from "./maintain-own-node-table-entry"

export interface PerSubnetParameters {
  subnetId: string
  subnetModule: SubnetModuleInstance
}

/**
 * Some effects are specific to each subnet so we export this helper which is called from the subnet instantiation code.
 */
export const runPerSubnetEffects = async (
  sig: EffectContext,
  parameters: PerSubnetParameters
) => {
  sig.run(handlePeerMessages, parameters)
  sig.run(calculateRoutes, parameters)
  await sig.run(maintainOwnNodeTableEntry, parameters)
}
