import { PlusIcon } from "lucide-react"
import { Link } from "wouter"

import { Button } from "../../components/ui/button"
import { rpc } from "../../utils/rpc"
import { LedgerListEntry } from "./ledger-list-entry"

export function LedgersPage() {
  const { data: ledgers } = rpc.ledgers.getList.useQuery()

  if (!ledgers) return <div>Loading...</div>

  return (
    <div className="container mx-auto max-w-xl flex flex-col gap-3">
      <h2 className="text-2xl font-bold tracking-tight">Active Ledgers</h2>
      {ledgers.map((ledger) => (
        <LedgerListEntry key={ledger.id} ledger={ledger} />
      ))}
      <Link href="/ledgers/create" asChild>
        <Button variant="outline" size="lg">
          <PlusIcon className="size-6 mr-2" />
          Add Ledger
        </Button>
      </Link>
    </div>
  )
}
