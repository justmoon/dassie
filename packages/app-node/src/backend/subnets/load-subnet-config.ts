import type { EffectContext } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { activeSubnetsSignal } from "./signals/active-subnets"

export const loadSubnetConfig = (sig: EffectContext) => {
  const { initialSubnets } = sig.use(configSignal).read()
  sig.use(activeSubnetsSignal).write(initialSubnets.map((subnet) => subnet.id))
}
