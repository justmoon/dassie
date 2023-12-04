import { createActor } from "@dassie/lib-reactive"

import { DassieActorContext } from "../base/types/dassie-base"
import { ManageSettlementSchemeInstancesActor } from "./manage-settlement-scheme-instances"
import { SendOutgoingSettlementsActor } from "./send-outgoing-settlements"

export const SettlementSchemesActor = () =>
  createActor((sig: DassieActorContext) => {
    sig.runMap(ManageSettlementSchemeInstancesActor)

    sig.runMap(SendOutgoingSettlementsActor)
  })
