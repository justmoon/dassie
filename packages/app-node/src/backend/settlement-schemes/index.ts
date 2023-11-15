import { createActor } from "@dassie/lib-reactive"

import { ManageSettlementSchemeInstancesActor } from "./manage-settlement-scheme-instances"
import { SendOutgoingSettlementsActor } from "./send-outgoing-settlements"

export const SettlementSchemesActor = () =>
  createActor((sig) => {
    sig.runMap(ManageSettlementSchemeInstancesActor)

    sig.runMap(SendOutgoingSettlementsActor)
  })
