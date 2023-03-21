import { createActor } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { activeSubnetsSignal } from "./signals/active-subnets"
import { primarySubnetSignal } from "./signals/primary-subnet"
import { subnetMapSignal } from "./signals/subnet-map"

export const loadSubnetConfig = () =>
  createActor((sig) => {
    const { initialSubnets } = sig.use(configSignal).read()
    const subnetMap = sig.get(subnetMapSignal)

    for (const initialSubnet of initialSubnets) {
      subnetMap.set(initialSubnet.id, {
        subnetId: initialSubnet.id,
        config: initialSubnet.config,
        initialPeers: initialSubnet.initialPeers ?? [],
      })
    }

    sig
      .use(activeSubnetsSignal)
      .write(initialSubnets.map((subnet) => subnet.id))

    // TODO: Primary subnet should be intelligently managed
    sig.use(primarySubnetSignal).write(initialSubnets[0]?.id)
  })
