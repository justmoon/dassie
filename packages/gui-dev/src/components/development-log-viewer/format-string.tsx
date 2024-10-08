import reactStringReplace from "react-string-replace"

import { TEST_NODE_VANITY_IDS } from "@dassie/app-dev/src/constants/vanity-nodes"
import { NODE_ID_REGEX } from "@dassie/app-dev/src/utils/shorten-node-id"
import { MAX_STRING_LENGTH } from "@dassie/gui-dassie/src/components/log-viewer/default-format"

import NodeLink from "../shared/node-link/node-link"

const vanityNodeIdsSet = new Set(TEST_NODE_VANITY_IDS)

export function formatString(value: string) {
  const chunks = reactStringReplace(value, NODE_ID_REGEX, (match, index) => {
    if (!vanityNodeIdsSet.has(match)) {
      return match
    }

    const nodeShortId = match.slice(0, match.indexOf("_"))
    return (
      <span key={index} className="font-bold">
        <NodeLink id={nodeShortId} />
      </span>
    )
  })

  let totalLength = 0

  for (let index = 0; index < chunks.length; index++) {
    const chunk = chunks[index]

    if (typeof chunk === "string") {
      if (totalLength + chunk.length > MAX_STRING_LENGTH) {
        chunks[index] = chunk.slice(0, MAX_STRING_LENGTH - totalLength) + "…"

        return chunks.slice(0, index + 1)
      }

      totalLength += chunk.length
    }
  }

  return chunks
}
