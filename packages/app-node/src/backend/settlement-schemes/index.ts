import { createActor } from "@dassie/lib-reactive"

import { ManageSettlementSchemeInstancesActor } from "./manage-settlement-scheme-instances"

export const SettlementSchemesActor = () =>
  createActor((sig) => {
    sig.runMap(ManageSettlementSchemeInstancesActor)
  })
