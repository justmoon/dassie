import type { Reactor } from "@dassie/lib-reactive"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { OwnerLedgerIdSignal } from "../../accounting/signals/owner-ledger-id"
import type { AccountPath } from "../../accounting/types/account-paths"
import type { EndpointInfo } from "./send-packet"

export function GetEndpointAccountPath(reactor: Reactor) {
  const ownerLedgerIdSignal = reactor.use(OwnerLedgerIdSignal)

  return function getEndpointAccountPath(
    endpointInfo: EndpointInfo,
  ): AccountPath {
    switch (endpointInfo.type) {
      case "peer": {
        return endpointInfo.accountPath
      }
      case "ildcp": {
        // ILDCP cannot send or receive money so we set the account path to an impossible value
        return "unreachable" as AccountPath
      }
      case "btp": {
        return `${ownerLedgerIdSignal.read()}:equity/owner`
      }
      case "local": {
        return `${ownerLedgerIdSignal.read()}:equity/owner`
      }
      case "http": {
        return `${ownerLedgerIdSignal.read()}:equity/owner`
      }
      default: {
        throw new UnreachableCaseError(endpointInfo)
      }
    }
  }
}
