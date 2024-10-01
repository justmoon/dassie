import { PenIcon, PlusIcon } from "lucide-react"

import { Amount } from "../../components/amount/amount"
import { Button } from "../../components/ui/button"
import { USD_SPECIFICATION } from "../../constants/currency"

export function LedgersPage() {
  return (
    <div className="container mx-auto max-w-xl flex flex-col gap-3">
      <div className="flex flex-col gap-3 border rounded-xl p-3 pl-4 xl:p-6 xl:pl-7">
        <div className="flex flex-row items-start justify-between">
          <div className="flex flex-row gap-3 items-center">
            <h4 className="text-xl font-bold">Ledger name</h4>
            <Button variant="ghost" size="icon" className="size-8">
              <PenIcon className="size-4" />
            </Button>
          </div>
          <div className="rounded-md bg-slate-800 text-slate-400 font-semibold px-2 py-1">
            Type
          </div>
        </div>
        <div className="flex flex-row gap-3">
          <Button>Send</Button>
          <Button>Receive</Button>
        </div>
        <div className="text-xl">
          <Amount value={0n} currency={USD_SPECIFICATION} />
        </div>
      </div>
      <Button variant="outline" size="lg">
        <PlusIcon className="size-6 mr-2" />
        Add Ledger
      </Button>
    </div>
  )
}
