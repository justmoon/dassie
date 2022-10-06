import type { EffectContext } from "@dassie/lib-reactive"

import { saveWalletToLocalStorage } from "./save-wallet-to-localstorage"

export const rootEffect = (sig: EffectContext) => {
  sig.run(saveWalletToLocalStorage)
}
