import { createClock } from "../generic/clock"
import type { Runtime } from "../types/runtime"
import { createCrypto } from "./crypto"

export const createRuntime = (): Runtime => {
  return {
    clock: createClock(),
    crypto: createCrypto(),
  }
}

export { createClock } from "../generic/clock"
export { createCrypto } from "./crypto"
