import { Redirect } from "wouter"

import type { SettlementSchemeId } from "@dassie/app-dassie/src/peer-protocol/types/settlement-scheme-id"

import { rpc } from "../../../utils/rpc"
import { DeleteLedgerAccount } from "./delete-ledger-account"

interface DeleteLedgerAccountProperties {
  params: { id: string }
}

export function DeleteLedgerAccountPage({
  params: { id },
}: DeleteLedgerAccountProperties) {
  const { data: basicState } = rpc.general.getBasicState.useQuery()

  if (!basicState) return <div>Loading...</div>

  if (
    !basicState.activeSettlementSchemes?.find((activeId) => activeId === id)
  ) {
    return <Redirect to="/ledgers" />
  }

  return <DeleteLedgerAccount id={id as SettlementSchemeId} />
}
