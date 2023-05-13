import { createActor } from "@dassie/lib-reactive"

import { ledgerStore } from "./stores/ledger"

export const manageBuiltinAccounts = () =>
  createActor((sig) => {
    const ledger = sig.use(ledgerStore)

    ledger.createAccount("internal/connector")
    ledger.createAccount("owner/spsp")
    ledger.createAccount("owner/btp")
  })
