import { entropyToMnemonic, mnemonicToSeedSync } from "@scure/bip39"
import { wordlist } from "@scure/bip39/wordlists/english"

import { writeFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { createInterface } from "node:readline"
import { fileURLToPath } from "node:url"

import { getPublicKey } from "@dassie/app-node/src/backend/crypto/ed25519"
import { calculatePathHmac } from "@dassie/app-node/src/backend/crypto/utils/seed-paths"
import { calculateNodeId } from "@dassie/app-node/src/backend/ilp-connector/utils/calculate-node-id"
import { SEED_PATH_NODE } from "@dassie/app-node/src/common/constants/seed-paths"

import {
  TEST_NODE_VANITY_ENTROPY,
  TEST_NODE_VANITY_SEEDS,
} from "../src/backend/constants/vanity-nodes"
import { nodeIndexToFriendlyId } from "../src/backend/utils/generate-node-config"

const TARGET_PATTERN = /^n([1-9]\d*)_/

const OUTPUT_FILE_PATH = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../src/backend/constants/vanity-nodes.ts"
)

const OUTPUT_FILE_HEADER = `// Generated with packages/app-dev/bin/add-testnet-vanity-addresses.ts
/* eslint-disable unicorn/numeric-separators-style */

`

const ENTROPY_SECTION_HEADER =
  "export const TEST_NODE_VANITY_ENTROPY: Record<number, string> = {\n"
const ENTROPY_SECTION_FOOTER = "}\n"

const MNEMONIC_SECTION_HEADER =
  "export const TEST_NODE_VANITY_MNEMONICS: Record<number, string> = {\n"
const MNEMONIC_SECTION_FOOTER = "}\n"

const SEED_SECTION_HEADER =
  "export const TEST_NODE_VANITY_SEEDS: Record<number, string> = {\n"
const SEED_SECTION_FOOTER = "}\n"

// Don't record keys for nodes above this index, unless it's the largest we've ever found (which we'll keep for fun)
const MAX_INDEX = 999

const foundMap = new Map<number, string>()
const seedCache = new Map<string, Buffer>()

for (const [indexAsString, key] of Object.entries(TEST_NODE_VANITY_ENTROPY)) {
  const index = Number.parseInt(indexAsString, 10)
  foundMap.set(index, key)

  const seedHex = TEST_NODE_VANITY_SEEDS[index]
  if (seedHex) {
    seedCache.set(key, Buffer.from(seedHex, "hex"))
  }
}

function saveResults() {
  let found = [...foundMap.entries()]
  found.sort(([a], [b]) => a - b)

  // Discard any indices above MAX_INDEX except for the record holder
  const recordHolder = found.at(-1)
  found = found.filter(
    ([index]) => index <= MAX_INDEX || index === recordHolder?.[0]
  )

  let entropyOutput = ENTROPY_SECTION_HEADER
  let mnemonicsOutput = MNEMONIC_SECTION_HEADER
  let seedsOutput = SEED_SECTION_HEADER
  for (const [index, key] of found) {
    const nodeEntropy = Buffer.from(key, "hex")
    const nodeId = getNodeIdFromEntropy(nodeEntropy)
    const mnemonic = entropyToMnemonic(nodeEntropy, wordlist)
    const nodeSeed = getNodeSeedFromEntropy(nodeEntropy)

    entropyOutput += `  ${index}: "${key}", // ${nodeId}\n`
    mnemonicsOutput += `  ${index}: "${mnemonic}",\n`
    seedsOutput += `  ${index}: "${nodeSeed.toString("hex")}",\n`
  }
  entropyOutput += ENTROPY_SECTION_FOOTER
  mnemonicsOutput += MNEMONIC_SECTION_FOOTER
  seedsOutput += SEED_SECTION_FOOTER

  const completeOutput = `${OUTPUT_FILE_HEADER}${[
    entropyOutput,
    mnemonicsOutput,
    seedsOutput,
  ].join("\n")}`
  writeFileSync(OUTPUT_FILE_PATH, completeOutput)
}

function getNodeSeedFromEntropy(entropy: Buffer) {
  const entropyHex = entropy.toString("hex")
  {
    const seed = seedCache.get(entropyHex)
    if (seed) return seed
  }
  const mnemonic = entropyToMnemonic(entropy, wordlist)
  const seed = Buffer.from(mnemonicToSeedSync(mnemonic))

  seedCache.set(entropyHex, seed)

  return seed
}

function getNodeIdFromEntropy(entropy: Buffer) {
  const seed = getNodeSeedFromEntropy(entropy)
  const nodePrivateKey = calculatePathHmac(seed, SEED_PATH_NODE)
  const nodePublicKey = getPublicKey(nodePrivateKey)
  return calculateNodeId(nodePublicKey)
}

function processSeed(seed: Buffer) {
  const nodeId = getNodeIdFromEntropy(seed)

  const match = nodeId.match(TARGET_PATTERN)
  if (!match) {
    throw new Error(`Invalid address: ${seed.toString("hex")} ${nodeId}`)
  }

  return { match, seed, nodeId }
}

function countHyphensAndUnderscores(seed: Buffer): number {
  const nodeId = getNodeIdFromEntropy(seed)

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

console.info("Incorporating vanity addresses...")

const rl = createInterface({
  input: process.stdin,
  output: process.stdout,
  terminal: false,
})

rl.on("line", (line) => {
  if (!line || line.startsWith("#")) return

  let seedHex = line.split(":")[0]!

  if (seedHex[1] === "x") seedHex = seedHex.slice(2)

  if (!seedHex) return

  const seed = Buffer.from(seedHex, "hex")

  const { match, nodeId } = processSeed(seed)
  const index = Number.parseInt(match[1]!, 10) - 1
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
})
