import { createActor } from "@dassie/lib-reactive"

import { handleSubnetModuleMessage } from "../peer-protocol/handlers/subnet-module-message"
import type { PerSubnetParameters } from "./manage-subnet-instances"

export const registerSubnetActor = () =>
  createActor((sig, { subnetId, subnetActor }: PerSubnetParameters) => {
    // Register subnet actor to handle incoming subnet module messages.
    sig.use(handleSubnetModuleMessage).tell("register", {
      subnetId,
      subnetActor,
    })

    // Unregister subnet actor when subnet is removed.
    sig.onCleanup(() => {
      sig.use(handleSubnetModuleMessage).tell("deregister", {
        subnetId,
      })
    })

    // Automatically re-register if the subnet module message handler is restarted.
    sig.subscribe(handleSubnetModuleMessage)
  })
