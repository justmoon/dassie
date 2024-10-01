import { createActor } from "@dassie/lib-reactive"

import type { DassieActorContext } from "../base/types/dassie-base"
import { ManageSettlementSchemeInstancesActor } from "./manage-settlement-scheme-instances"
import { SendOutgoingSettlementsActor } from "./send-outgoing-settlements"

export const LedgersActor = () =>
  createActor(async (sig: DassieActorContext) => {
    await Promise.all(sig.runMap(ManageSettlementSchemeInstancesActor))

    sig.runMap(SendOutgoingSettlementsActor)
  })
