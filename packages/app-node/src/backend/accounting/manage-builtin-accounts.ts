import { createActor } from "@dassie/lib-reactive"

import { SubnetId } from "../peer-protocol/types/subnet-id"
import { initializeCommonAccounts } from "./functions/manage-common-accounts"
import { ledgerStore } from "./stores/ledger"

export const manageBuiltinAccounts = () =>
  createActor((sig) => {
    const ledger = sig.use(ledgerStore)

    initializeCommonAccounts(ledger, "builtin" as SubnetId)
    ledger.createAccount(`builtin/owner/spsp`)
    ledger.createAccount(`builtin/owner/btp`)
  })
