import type { EffectContext } from "@dassie/lib-reactive"

import * as modules from "./modules"
import { activeSubnetsSignal } from "./signals/active-subnets"
import { subnetMapSignal } from "./signals/subnet-map"
import type { Subnet } from "./types/subnet-module"

export const manageSubnetInstances = async (sig: EffectContext) => {
  await Promise.all(sig.for(activeSubnetsSignal, runSubnetModule))
}

const runSubnetModule = async (sig: EffectContext, subnetId: string) => {
  const subnetMap = sig.get(subnetMapSignal)

  const createModule = (modules as Record<string, Subnet>)[subnetId]
  if (!createModule) {
    throw new Error(`Unknown subnet module '${subnetId}'`)
  }

  const instance = await createModule()

  subnetMap.set(subnetId, instance)

  sig.onCleanup(() => {
    subnetMap.delete(subnetId)
  })
}
