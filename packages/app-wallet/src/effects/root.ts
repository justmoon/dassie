import type { EffectContext } from "@dassie/lib-reactive"

import { automaticallyCreateFirstUplinks } from "./automatically-create-first-uplinks"
import { saveWalletToLocalStorage } from "./save-wallet-to-localstorage"

export const rootEffect = (sig: EffectContext) => {
  sig.run(saveWalletToLocalStorage)
  void sig.run(automaticallyCreateFirstUplinks)
}
