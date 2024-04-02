import { uint8ArrayToHex } from "uint8array-extras"

import { createSignal } from "@dassie/lib-reactive"

import type { DassieReactor } from "../../base/types/dassie-base"

export const SetupAuthorizationTokenSignal = (reactor: DassieReactor) => {
  const { random } = reactor.base

  return createSignal(uint8ArrayToHex(random.randomBytes(32)))
}
