import { Infer } from "@dassie/lib-oer"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { nodeInfoEntry } from "../peer-schema"

export type LinkStateEntry = Infer<typeof nodeInfoEntry>

export const parseLinkStateEntries = (entries: LinkStateEntry[]) => {
  const neighbors = []
  const subnets = []

  for (const entry of entries) {
    switch (entry.type) {
      case "neighbor": {
        neighbors.push(entry.value.nodeId)
        break
      }
      case "subnet": {
        subnets.push(entry.value.subnetId)
        break
      }
      default: {
        throw new UnreachableCaseError(entry)
      }
    }
  }

  return {
    neighbors,
    subnets,
  }
}
