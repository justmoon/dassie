import { createSignal } from "@dassie/lib-reactive"

import { castLedgerId } from "../utils/cast-ledger-id"

/**
 * Determines which internal ledger the owner accounts are on.
 *
 * @remarks
 *
 * Dassie aims for multi-currency support but in this current version, the owner accounts are only on one ledger. This
 * signal determines which ledger that is.
 *
 * Owner accounts are the accounts that are used when the node's owner sends or receives money or connects a client via
 * BTP etc.
 */
export const OwnerLedgerIdSignal = () => createSignal(castLedgerId("stub+usd"))
