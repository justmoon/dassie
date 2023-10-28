import { createActor } from "@dassie/lib-reactive"

import { SettlementSchemeId } from "../peer-protocol/types/settlement-scheme-id"
import { initializeCommonAccounts } from "./functions/manage-common-accounts"
import { LedgerStore } from "./stores/ledger"

export const ManageBuiltinAccountsActor = () =>
  createActor((sig) => {
    const ledger = sig.use(LedgerStore)

    initializeCommonAccounts(ledger, "builtin" as SettlementSchemeId)
    ledger.createAccount(`builtin/owner/spsp`)
    ledger.createAccount(`builtin/owner/btp`)
  })
