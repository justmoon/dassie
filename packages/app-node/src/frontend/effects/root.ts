import { createActor } from "@dassie/lib-reactive"

import { saveWalletToLocalStorage } from "./save-wallet-to-localstorage"

export const rootActor = () =>
  createActor((sig) => {
    sig.run(saveWalletToLocalStorage)
  })
