import { createActor } from "@dassie/lib-reactive"

import { calculateRoutes } from "./calculate-routes"
import { maintainOwnNodeTableEntry } from "./maintain-own-node-table-entry"
import { maintainPeeringRelationships } from "./maintain-peering-relationships"
import { queueBootstrapNodes } from "./queue-bootstrap-node"

export interface PerSubnetParameters {
  subnetId: string
}

/**
 * Some effects are specific to each subnet so we export this helper which is called from the subnet instantiation code.
 */
export const runPerSubnetEffects = () =>
  createActor(async (sig, parameters: PerSubnetParameters) => {
    sig.run(calculateRoutes, parameters)
    await sig.run(maintainOwnNodeTableEntry, parameters)
    sig.run(maintainPeeringRelationships, parameters)
    sig.run(queueBootstrapNodes, parameters)
  })
