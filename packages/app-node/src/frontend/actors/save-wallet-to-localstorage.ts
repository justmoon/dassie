import { createActor } from "@dassie/lib-reactive"

import { walletStore } from "../stores/wallet"

export const saveWalletToLocalStorage = () =>
  createActor((sig) => {
    const wallet = sig.get(walletStore)

    localStorage.setItem("wallet", JSON.stringify(wallet))
  })
