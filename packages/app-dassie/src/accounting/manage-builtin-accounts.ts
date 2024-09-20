import { createActor } from "@dassie/lib-reactive"

import { initializeCommonAccounts } from "./functions/manage-common-accounts"
import { OwnerLedgerIdSignal } from "./signals/owner-ledger-id"
import { LedgerStore } from "./stores/ledger"

export const ManageBuiltinAccountsActor = () =>
  createActor((sig) => {
    const ledger = sig.reactor.use(LedgerStore)
    const ledgerId = sig.read(OwnerLedgerIdSignal)

    initializeCommonAccounts(ledger, ledgerId)
  })
