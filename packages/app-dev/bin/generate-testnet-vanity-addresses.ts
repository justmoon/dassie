import { randomBytes } from "node:crypto"
import { writeFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

import { getPublicKey } from "@dassie/app-node/src/backend/crypto/ed25519"
import { calculateNodeId } from "@dassie/app-node/src/backend/ilp-connector/utils/calculate-node-id"

import { TEST_NODE_VANITY_KEYS } from "../src/backend/constants/node-keys"
import { nodeIndexToFriendlyId } from "../src/backend/utils/generate-node-config"

const TARGET_PATTERN = /^n([1-9]\d*)_/

const OUTPUT_FILE_PATH = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../src/backend/constants/node-keys.ts"
)

const OUTPUT_FILE_HEADER = `// Generated with packages/app-dev/bin/generate-testnet-vanity-addresses.ts
/* eslint-disable unicorn/numeric-separators-style */

export const TEST_NODE_VANITY_KEYS: Record<number, string> = {
`

const OUTPUT_FILE_FOOTER = `}`

// Don't record keys for nodes above this index, unless it's the largest we've ever found (which we'll keep for fun)
const MAX_INDEX = 999

const foundMap = new Map<number, string>()

for (const [index, key] of Object.entries(TEST_NODE_VANITY_KEYS)) {
  foundMap.set(Number.parseInt(index, 10), key)
}

function saveResults() {
  let found = [...foundMap.entries()]
  found.sort(([a], [b]) => a - b)

  // Discard any indices above MAX_INDEX except for the record holder
  const recordHolder = found[found.length - 1]
  found = found.filter(
    ([index]) => index <= MAX_INDEX || index === recordHolder?.[0]
  )

  let output = OUTPUT_FILE_HEADER
  for (const [index, key] of found) {
    const nodePrivateKey = Buffer.from(key, "hex")
    const nodePublicKey = getPublicKey(nodePrivateKey)
    const nodeId = calculateNodeId(nodePublicKey)
    output += `  ${index}: "${key}", // ${nodeId}\n`
  }
  output += OUTPUT_FILE_FOOTER

  writeFileSync(OUTPUT_FILE_PATH, output)
}

function findNextMatch() {
  for (;;) {
    const nodePrivateKey = randomBytes(32)
    const nodePublicKey = getPublicKey(nodePrivateKey)
    const nodeId = calculateNodeId(nodePublicKey)

    const match = nodeId.match(TARGET_PATTERN)
    if (match) return { match, nodePrivateKey, nodeId }
  }
}

function countHyphensAndUnderscores(key: string): number {
  const nodePrivateKey = Buffer.from(key, "hex")
  const nodePublicKey = getPublicKey(nodePrivateKey)
  const nodeId = calculateNodeId(nodePublicKey)

  let count = 0

  for (const char of nodeId) {
    if (char === "-" || char === "_") {
      count++
    }
  }

  return count
}

function isLargestEver(index: number) {
  for (const existingIndex of foundMap.keys()) {
    if (index <= existingIndex) return false
  }

  return true
}

/**
 * Checks if a newly generated vanity address is new or better.
 *
 * @remarks
 *
 * The address is new if we don't have any vanity key for the given index. It is better if it has fewer hyphens and underscores than the existing one.
 */
function isNewOrBetter(index: number, key: string) {
  const existing = foundMap.get(index)
  if (!existing) return true

  const existingHyphenCount = countHyphensAndUnderscores(existing)
  const hyphenCount = countHyphensAndUnderscores(key)

  return hyphenCount < existingHyphenCount
}

console.log("Generating vanity addresses... (press Ctrl+C to stop)")
for (;;) {
  const { match, nodePrivateKey, nodeId } = findNextMatch()
  const index = Number.parseInt(match[1]!, 10) - 1
  const key = Buffer.from(nodePrivateKey).toString("hex")
  const friendlyId = nodeIndexToFriendlyId(index)

  if (index > MAX_INDEX) {
    if (isLargestEver(index)) {
      foundMap.set(index, key)
      console.log(`${friendlyId} => ${nodeId} (new record!)`)
      saveResults()
    } else {
      console.log(`${friendlyId} => ${nodeId} (skipped, too large)`)
    }
  } else if (isNewOrBetter(index, key)) {
    foundMap.set(index, key)
    console.log(`${friendlyId} => ${nodeId}`)
    saveResults()
  }
}
