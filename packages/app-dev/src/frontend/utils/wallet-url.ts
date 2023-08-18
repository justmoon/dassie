import { TEST_NODE_VANITY_SEEDS } from "../../backend/constants/node-seeds"

export const getWalletUrl = (nodeId: string) => {
  const walletUrlBase = `https://${nodeId}.localhost`
  const nodeIndex = Number.parseInt(nodeId.slice(1)) - 1

  const seed = TEST_NODE_VANITY_SEEDS[nodeIndex]

  if (!seed) {
    throw new Error("No vanity seed found for node " + nodeId)
  }

  return `${walletUrlBase}/?_DASSIE_DEV_SEED=${seed}`
}
