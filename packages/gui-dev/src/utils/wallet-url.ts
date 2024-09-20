import { bytesToHex, hexToBytes } from "@noble/hashes/utils"

import { SEED_PATH_DEV_SESSION } from "@dassie/app-dassie/src/constants/seed-paths"
import { QUERY_PARAMETER_DEVELOPMENT_SESSION } from "@dassie/app-dassie/src/constants/ui-query-parameter-names"
import { TEST_NODE_VANITY_SEEDS } from "@dassie/app-dev/src/constants/vanity-nodes"
import { getPrivateSeedAtPath } from "@dassie/gui-dassie/src/utils/signer"

export const getWalletUrl = (nodeId: string) => {
  const walletUrlBase = `https://${nodeId}.localhost`
  const nodeIndex = Number.parseInt(nodeId.slice(1)) - 1

  const seed = TEST_NODE_VANITY_SEEDS[nodeIndex]

  if (!seed) {
    throw new Error("No vanity seed found for node " + nodeId)
  }

  const sessionToken = bytesToHex(
    getPrivateSeedAtPath(hexToBytes(seed), SEED_PATH_DEV_SESSION),
  )

  return `${walletUrlBase}/?${QUERY_PARAMETER_DEVELOPMENT_SESSION}=${sessionToken}`
}
