import { Amount } from "../../../components/amount/amount"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "../../../components/ui/table"
import { USD_SPECIFICATION } from "../../../constants/currency"
import { trpc } from "../../../utils/trpc"

export function Ledger() {
  const ledger = trpc.getLedger.useQuery().data ?? []

  return (
    <div>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead className="w-[100px]">Account</TableHead>
            <TableHead className="text-right">Balance</TableHead>
            <TableHead className="text-right">Credits</TableHead>
            <TableHead className="text-right">Debits</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {ledger.map(
            ({ path, debitsPending, creditsPosted, debitsPosted }) => {
              const balance = creditsPosted - debitsPosted - debitsPending
              return (
                <TableRow key={path}>
                  <TableCell className="font-medium">{path}</TableCell>
                  <TableCell className="text-right">
                    <Amount value={balance} currency={USD_SPECIFICATION} />
                  </TableCell>
                  <TableCell className="text-right">
                    <Amount
                      value={creditsPosted}
                      currency={USD_SPECIFICATION}
                    />
                  </TableCell>
                  <TableCell className="text-right">
                    <Amount value={debitsPosted} currency={USD_SPECIFICATION} />
                  </TableCell>
                </TableRow>
              )
            }
          )}
        </TableBody>
      </Table>
    </div>
  )
}
