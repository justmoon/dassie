import { SubnetId } from "../../peer-protocol/types/subnet-id"
import { Ledger } from "../stores/ledger"

export const initializeCommonAccounts = (
  ledger: Ledger,
  subnetId: SubnetId
) => {
  ledger.createAccount(`${subnetId}/internal/connector`)
}
