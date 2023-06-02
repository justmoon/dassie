import { createActor } from "@dassie/lib-reactive"

import { loadSubnetConfig } from "./load-subnet-config"
import { manageSubnetInstances } from "./manage-subnet-instances"

export const startSubnets = () =>
  createActor((sig) => {
    sig.run(loadSubnetConfig)
    sig.runMap(manageSubnetInstances)
  })
