import type { EffectContext } from "@dassie/lib-reactive"

import { walletStore } from "../stores/wallet"

export const saveWalletToLocalStorage = (sig: EffectContext) => {
  const wallet = sig.get(walletStore)

  localStorage.setItem("wallet", JSON.stringify(wallet))
}
