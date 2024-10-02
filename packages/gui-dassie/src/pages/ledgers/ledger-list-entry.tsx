import type { SettlementSchemeId } from "@dassie/app-dassie/src/peer-protocol/types/settlement-scheme-id"

import { Amount } from "../../components/amount/amount"
import { Button } from "../../components/ui/button"
import { SCHEME_CURRENCY_MAP, SCHEME_NAME_MAP } from "../../constants/schemes"

interface LedgerListEntryProperties {
  ledger: {
    id: SettlementSchemeId
    balance: bigint
  }
}

export function LedgerListEntry({
  ledger: { id, balance },
}: LedgerListEntryProperties) {
  return (
    <div className="flex flex-col gap-3 border rounded-xl p-3 pl-4 xl:p-6 xl:pl-7">
      <div className="flex flex-row gap-3 items-center">
        <h4 className="text-xl font-bold">{SCHEME_NAME_MAP[id]}</h4>
      </div>
      <div className="flex flex-row gap-3">
        <Button>Send</Button>
        <Button>Receive</Button>
      </div>
      <div className="text-xl">
        <Amount value={balance} currency={SCHEME_CURRENCY_MAP[id]} />
      </div>
    </div>
  )
}
