import {
  ChevronsRightIcon,
  MoveDownRightIcon,
  MoveUpRightIcon,
} from "lucide-react"
import { useState } from "react"
import { Link } from "wouter"

import type {
  LedgerAccount,
  Transfer,
} from "../../../../backend/accounting/stores/ledger"
import { Button } from "../../../components/ui/button"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "../../../components/ui/table"
import { combine } from "../../../utils/class-helper"
import { trpc } from "../../../utils/trpc"

export interface AccountDetailPageProperties {
  readonly params: { readonly ledgerId: string; readonly accountPath: string }
}

export const AccountDetailPage = ({
  params: { ledgerId, accountPath },
}: AccountDetailPageProperties) => {
  const fullAccountPath = `${ledgerId}:${accountPath}`

  trpc.debug.subscribeToLedgerAccount.useSubscription(fullAccountPath, {
    onData: (data) => {
      if ("path" in data) {
        setInitialAccountState(data)
      } else {
        setTransfers((transfers) => [...transfers, data])
      }
    },
  })

  const [initialAccountState, setInitialAccountState] = useState<
    LedgerAccount | undefined
  >(undefined)
  const [transfers, setTransfers] = useState<Transfer[]>([])

  if (!initialAccountState) {
    return <div>Loading...</div>
  }

  let runningBalance =
    initialAccountState.creditsPosted -
    initialAccountState.debitsPosted -
    initialAccountState.debitsPending

  return (
    <div>
      <div className="flex flex-row items-center">
        <Button variant="link" asChild className="px-2">
          <Link href={"/ledger"}>{ledgerId}</Link>
        </Button>
        <ChevronsRightIcon className="h-4 w-4 mr-2" />
        {accountPath}
      </div>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>DR/CR</TableHead>
            <TableHead>Account</TableHead>
            <TableHead>Amount</TableHead>
            <TableHead>Balance</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {transfers.map((transfer, index) => {
            if (
              (transfer.creditAccount === fullAccountPath &&
                transfer.state === "posted") ||
              (transfer.debitAccount === fullAccountPath &&
                transfer.state === "voided")
            ) {
              runningBalance += transfer.amount
            }

            if (
              (transfer.debitAccount === fullAccountPath &&
                transfer.state === "posted" &&
                transfer.immediate) ||
              (transfer.debitAccount === fullAccountPath &&
                transfer.state === "pending")
            ) {
              runningBalance -= transfer.amount
            }

            return (
              <TableRow
                key={index}
                className={combine(
                  transfer.state === "pending" ? "opacity-50" : "",
                  transfer.state === "voided" ? "opacity-50 line-through" : "",
                )}
              >
                {transfer.creditAccount === fullAccountPath ? (
                  <TableCell>
                    <MoveDownRightIcon className="h-4 w-4 inline-block mr-2" />
                    CR
                  </TableCell>
                ) : (
                  <TableCell>
                    <MoveUpRightIcon className="h-4 w-4 inline-block mr-2" />
                    DR
                  </TableCell>
                )}
                <TableCell>
                  {transfer.creditAccount === fullAccountPath
                    ? transfer.debitAccount
                    : transfer.creditAccount}
                </TableCell>
                <TableCell>{String(transfer.amount)}</TableCell>
                <TableCell>{String(runningBalance)}</TableCell>
              </TableRow>
            )
          })}
          {transfers.length === 0 ? (
            <p className="p-4">Waiting for transfers to occur...</p>
          ) : null}
        </TableBody>
      </Table>
    </div>
  )
}
