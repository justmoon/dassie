import { createActor } from "@dassie/lib-reactive"

import { DassieActorContext } from "../base/types/dassie-base"
import { HandleIldcpRequestsActor } from "./handle-ildcp-requests"

export const IldcpServerActor = () =>
  createActor((sig: DassieActorContext) => {
    sig.run(HandleIldcpRequestsActor)
  })
