import type { Transfer } from "../../accounting/stores/ledger"

export const PendingSettlementsMap = () => new Map<string, Transfer>()
