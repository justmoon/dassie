import type { Scenario } from "./common"
import * as crossCurrency from "./cross-currency"
import * as sixNodes from "./six-nodes"
import * as twoNodes from "./two-nodes"
import * as xrplSettlement from "./xrpl-settlement"

export const scenarios = {
  "two-nodes": twoNodes,
  "six-nodes": sixNodes,
  "xrpl-settlement": xrplSettlement,
  "cross-currency": crossCurrency,
} satisfies Record<string, Scenario>
