import type { Clock, Random } from "@dassie/lib-reactive"

import { NodeClockImplementation } from "./clock"
import { NodeRandomImplementation } from "./random"

export interface NodeRuntime {
  random: Random
  clock: Clock
}

export const createNodeRuntime = (): NodeRuntime => {
  return {
    random: new NodeRandomImplementation(),
    clock: new NodeClockImplementation(),
  }
}
