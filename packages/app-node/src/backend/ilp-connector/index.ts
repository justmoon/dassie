import { createActor } from "@dassie/lib-reactive"

import { DassieActorContext } from "../base/types/dassie-base"
import { ProcessPacketActor } from "./process-packet"

export const IlpConnectorActor = () =>
  createActor((sig: DassieActorContext) => {
    sig.run(ProcessPacketActor)
  })
