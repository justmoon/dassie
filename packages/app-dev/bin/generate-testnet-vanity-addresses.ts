import { randomBytes } from "node:crypto"
import { writeFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

import { getPublicKey } from "@dassie/app-node/src/backend/crypto/ed25519"
import { calculatePathHmac } from "@dassie/app-node/src/backend/crypto/utils/seed-paths"
import { calculateNodeId } from "@dassie/app-node/src/backend/ilp-connector/utils/calculate-node-id"
import { SEED_PATH_NODE } from "@dassie/app-node/src/common/constants/seed-paths"

import { TEST_NODE_VANITY_SEEDS } from "../src/backend/constants/node-seeds"
import { nodeIndexToFriendlyId } from "../src/backend/utils/generate-node-config"

const TARGET_PATTERN = /^n([1-9]\d*)_/

const OUTPUT_FILE_PATH = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../src/backend/constants/node-seeds.ts"
)

const OUTPUT_FILE_HEADER = `// Generated with packages/app-dev/bin/generate-testnet-vanity-addresses.ts
/* eslint-disable unicorn/numeric-separators-style */

export const TEST_NODE_VANITY_SEEDS: Record<number, string> = {
`

const OUTPUT_FILE_FOOTER = `}`

// Don't record keys for nodes above this index, unless it's the largest we've ever found (which we'll keep for fun)
const MAX_INDEX = 999

const foundMap = new Map<number, string>()

for (const [index, key] of Object.entries(TEST_NODE_VANITY_SEEDS)) {
  foundMap.set(Number.parseInt(index, 10), key)
}

function saveResults() {
  let found = [...foundMap.entries()]
  found.sort(([a], [b]) => a - b)

  // Discard any indices above MAX_INDEX except for the record holder
  const recordHolder = found.at(-1)
  found = found.filter(
    ([index]) => index <= MAX_INDEX || index === recordHolder?.[0]
  )

  let output = OUTPUT_FILE_HEADER
  for (const [index, key] of found) {
    const nodeSeed = Buffer.from(key, "hex")
    const nodeId = getNodeIdFromSeed(nodeSeed)
    output += `  ${index}: "${key}", // ${nodeId}\n`
  }
  output += OUTPUT_FILE_FOOTER

  writeFileSync(OUTPUT_FILE_PATH, output)
}

function getNodeIdFromSeed(seed: Buffer) {
  const nodePrivateKey = calculatePathHmac(seed, SEED_PATH_NODE)
  const nodePublicKey = getPublicKey(nodePrivateKey)
  return calculateNodeId(nodePublicKey)
}

function findNextMatch() {
  for (;;) {
    const seed = randomBytes(32)
    const nodeId = getNodeIdFromSeed(seed)

    const match = nodeId.match(TARGET_PATTERN)
    if (match) return { match, seed, nodeId }
  }
}

function countHyphensAndUnderscores(seed: Buffer): number {
  const nodeId = getNodeIdFromSeed(seed)

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
function isNewOrBetter(index: number, seed: Buffer) {
  const existing = foundMap.get(index)
  if (!existing) return true

  const existingHyphenCount = countHyphensAndUnderscores(
    Buffer.from(existing, "hex")
  )
  const hyphenCount = countHyphensAndUnderscores(seed)

  return hyphenCount < existingHyphenCount
}

console.info("Generating vanity addresses... (press Ctrl+C to stop)")
for (;;) {
  const { match, seed, nodeId } = findNextMatch()
  const index = Number.parseInt(match[1]!, 10) - 1
  const seedHex = seed.toString("hex")
  const friendlyId = nodeIndexToFriendlyId(index)

  if (index > MAX_INDEX) {
    if (isLargestEver(index)) {
      foundMap.set(index, seedHex)
      console.info(`${friendlyId} => ${nodeId} (new record!)`)
      saveResults()
    } else {
      console.info(`${friendlyId} => ${nodeId} (skipped, too large)`)
    }
  } else if (isNewOrBetter(index, seed)) {
    foundMap.set(index, seedHex)
    console.info(`${friendlyId} => ${nodeId}`)
    saveResults()
  }
}
