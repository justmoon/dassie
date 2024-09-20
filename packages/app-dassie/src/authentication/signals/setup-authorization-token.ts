import { uint8ArrayToHex } from "uint8array-extras"

import { createSignal } from "@dassie/lib-reactive"

import type { DassieReactor } from "../../base/types/dassie-base"

export const SetupAuthorizationTokenSignal = (reactor: DassieReactor) => {
  const { crypto } = reactor.base

  return createSignal(uint8ArrayToHex(crypto.getRandomBytes(32)))
}
