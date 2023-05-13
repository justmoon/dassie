import { createActor } from "@dassie/lib-reactive"

import { manageBuiltinAccounts } from "./manage-builtin-accounts"

export const startAccounting = () =>
  createActor((sig) => {
    sig.run(manageBuiltinAccounts)
  })
