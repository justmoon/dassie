import type { EffectContext } from "@dassie/lib-reactive"

import { loadSubnetConfig } from "./load-subnet-config"
import { manageSubnetInstances } from "./manage-subnet-instances"

export const startSubnets = async (sig: EffectContext) => {
  sig.run(loadSubnetConfig)
  await sig.run(manageSubnetInstances)
}
