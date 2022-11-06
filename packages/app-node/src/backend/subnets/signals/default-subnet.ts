import { createSignal } from "@dassie/lib-reactive"

import type { SubnetModule } from "../types/subnet-module"

export const defaultSubnetSignal = () =>
  createSignal<SubnetModule | undefined>()
