import { Time } from "@dassie/lib-reactive"

import { NodeTimeImplementation } from "./time"

interface NodeRuntime {
  time: Time
}

export const createNodeRuntime = (): NodeRuntime => {
  return {
    time: new NodeTimeImplementation(),
  }
}
