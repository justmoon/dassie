import { createActor } from "@dassie/lib-reactive"

import { loadSettlementSchemeConfig } from "./load-settlement-scheme-config"
import { manageSettlementSchemeInstances } from "./manage-settlement-scheme-instances"

export const startSettlementSchemes = () =>
  createActor((sig) => {
    sig.run(loadSettlementSchemeConfig)
    sig.runMap(manageSettlementSchemeInstances)
  })
