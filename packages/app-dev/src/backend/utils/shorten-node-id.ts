import { TEST_NODE_VANITY_IDS } from "../constants/vanity-nodes"

export const NODE_ID_REGEX = /(d\d{1,4}_[\w-]{25,30})/

const vanityNodeIdsSet = new Set(TEST_NODE_VANITY_IDS)

export function shortenNodeId(nodeId: string) {
  if (!vanityNodeIdsSet.has(nodeId)) {
    return nodeId
  }

  return nodeId.slice(0, nodeId.indexOf("_"))
}
