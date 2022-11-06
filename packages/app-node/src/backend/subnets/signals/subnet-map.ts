import { createSignal } from "@dassie/lib-reactive"

import type { SubnetModule } from "../types/subnet-module"

export const subnetMapSignal = () =>
  createSignal<Map<string, SubnetModule>>(new Map())
