import { createClock } from "../generic/clock"
import type { Runtime } from "../types/runtime"
import { createRandom } from "./random"

export const createRuntime = (): Runtime => {
  return {
    random: createRandom(),
    clock: createClock(),
  }
}

export { createRandom } from "./random"
export { createClock } from "../generic/clock"
