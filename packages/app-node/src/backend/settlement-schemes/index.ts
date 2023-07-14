import { createActor } from "@dassie/lib-reactive"

import { manageSettlementSchemeInstances } from "./manage-settlement-scheme-instances"

export const startSettlementSchemes = () =>
  createActor((sig) => {
    sig.runMap(manageSettlementSchemeInstances)
  })
