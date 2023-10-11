import { createActor } from "@dassie/lib-reactive"

import { ManageBuiltinAccountsActor } from "./manage-builtin-accounts"

export const AccountingActor = () =>
  createActor((sig) => {
    sig.run(ManageBuiltinAccountsActor)
  })
