import type { EffectContext } from "@dassie/lib-reactive"

import { saveWalletToLocalStorage } from "./save-wallet-to-localstorage"
import { maintainUplink } from "./uplink/maintain-uplink"

export const rootEffect = (sig: EffectContext) => {
  sig.run(saveWalletToLocalStorage)
  sig.run(maintainUplink)
}
