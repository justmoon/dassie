import type { Random, Time } from "@dassie/lib-reactive"

import { NodeRandomImplementation } from "./random"
import { NodeTimeImplementation } from "./time"

interface NodeRuntime {
  random: Random
  time: Time
}

export const createNodeRuntime = (): NodeRuntime => {
  return {
    random: new NodeRandomImplementation(),
    time: new NodeTimeImplementation(),
  }
}
