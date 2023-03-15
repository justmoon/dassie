import type { EffectContext } from "@dassie/lib-reactive"

import { calculateRoutes } from "./calculate-routes"
import { maintainOwnNodeTableEntry } from "./maintain-own-node-table-entry"

export interface PerSubnetParameters {
  subnetId: string
}

/**
 * Some effects are specific to each subnet so we export this helper which is called from the subnet instantiation code.
 */
export const runPerSubnetEffects = async (
  sig: EffectContext,
  parameters: PerSubnetParameters
) => {
  sig.run(calculateRoutes, parameters)
  await sig.run(maintainOwnNodeTableEntry, parameters)
}
