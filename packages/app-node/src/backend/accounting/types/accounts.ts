import { NodeId } from "../../peer-protocol/types/node-id"
import { LedgerId } from "./ledger-id"

// Ledger Account Structure
// ------------------------
//
// internal/… -> Own accounts
// internal/connector -> Connector account
// peer/… -> Peer accounts
// peer/<nodeId> -> Accounts for a specific peer
// peer/<nodeId>/interledger -> Interledger packets
// peer/<nodeId>/settlement -> Underlying settlement account
// peer/<nodeId>/trust -> Trust extended or revoked
// owner/… -> Owner accounts
// owner/spsp/<spspAccountId> -> SPSP accounts
// owner/btp/<btpAccountId> -> BTP accounts
// owner/http/<httpAccountId> -> ILP-HTTP accounts

export type ConnectorAccount = `${LedgerId}:internal/connector`

export type PeerInterledgerAccount = `${LedgerId}:peer/${NodeId}/interledger`
export type PeerSettlementAccount = `${LedgerId}:peer/${NodeId}/settlement`
export type PeerTrustAccount = `${LedgerId}:peer/${NodeId}/trust`

export type OwnerSpspAccount = `${LedgerId}:owner/spsp`
export type OwnerBtpAccount = `${LedgerId}:owner/btp`
export type OwnerHttpAccount = `${LedgerId}:owner/http`

export type AccountPath =
  | ConnectorAccount
  | PeerInterledgerAccount
  | PeerSettlementAccount
  | PeerTrustAccount
  | OwnerSpspAccount
  | OwnerBtpAccount
  | OwnerHttpAccount
