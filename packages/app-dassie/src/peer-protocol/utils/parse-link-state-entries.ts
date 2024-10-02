import type { Infer } from "@dassie/lib-oer"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { nodeInfoEntry } from "../peer-schema"
import type { SettlementSchemeId } from "../types/settlement-scheme-id"

export type LinkStateEntry = Infer<typeof nodeInfoEntry>

export const parseLinkStateEntries = (entries: LinkStateEntry[]) => {
  const neighbors = []
  const settlementSchemes: SettlementSchemeId[] = []

  for (const entry of entries) {
    switch (entry.type) {
      case "neighbor": {
        neighbors.push(entry.value.nodeId)
        break
      }
      case "settlementScheme": {
        settlementSchemes.push(entry.value.settlementSchemeId)
        break
      }
      default: {
        throw new UnreachableCaseError(entry)
      }
    }
  }

  return {
    neighbors,
    settlementSchemes,
  }
}
