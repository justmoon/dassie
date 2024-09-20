import { entropyToMnemonic, mnemonicToSeedSync } from "@scure/bip39"
import { wordlist } from "@scure/bip39/wordlists/english"

import assert from "node:assert"
import { writeFileSync } from "node:fs"
import path from "node:path"
import { createInterface } from "node:readline"
import { fileURLToPath } from "node:url"

import { SEED_PATH_NODE } from "@dassie/app-dassie/src/constants/seed-paths"
import { getPublicKey } from "@dassie/app-dassie/src/crypto/ed25519"
import { calculatePathHmac } from "@dassie/app-dassie/src/crypto/utils/seed-paths"
import { calculateNodeId } from "@dassie/app-dassie/src/ilp-connector/utils/calculate-node-id"

import {
  TEST_NODE_VANITY_ENTROPY,
  TEST_NODE_VANITY_SEEDS,
} from "../src/constants/vanity-nodes"
import { nodeIndexToFriendlyId } from "../src/utils/generate-node-config"

const TARGET_PATTERN = /^d([1-9]\d*)_/

const OUTPUT_FILE_PATH = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../src/constants/vanity-nodes.ts",
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

const ID_SECTION_HEADER = "export const TEST_NODE_VANITY_IDS: string[] = [\n"
const ID_SECTION_FOOTER = "]\n"

// Don't record keys for nodes above this index, unless it's the largest we've ever found (which we'll keep for fun)
const MAX_INDEX = 999

const foundMap = new Map<number, { seed: string; nodeId: string }>()
const seedCache = new Map<string, Buffer>()

console.info("Loading existing known vanity addresses...")
for (const [indexAsString, key] of Object.entries(TEST_NODE_VANITY_ENTROPY)) {
  const index = Number.parseInt(indexAsString, 10)
  foundMap.set(index, {
    seed: key,
    nodeId: getNodeIdFromEntropy(Buffer.from(key, "hex")),
  })

  const seedHex = TEST_NODE_VANITY_SEEDS[index]
  if (seedHex) {
    seedCache.set(key, Buffer.from(seedHex, "hex"))
  }
}

saveResults()

function saveResults() {
  let found = [...foundMap.entries()]
  found.sort(([a], [b]) => a - b)

  // Discard any indices above MAX_INDEX except for the record holder
  const recordHolder = found.at(-1)
  found = found.filter(
    ([index]) => index <= MAX_INDEX || index === recordHolder?.[0],
  )

  let entropyOutput = ENTROPY_SECTION_HEADER
  let mnemonicsOutput = MNEMONIC_SECTION_HEADER
  let seedsOutput = SEED_SECTION_HEADER
  let idsOutput = ID_SECTION_HEADER
  for (const [index, { seed, nodeId }] of found) {
    const nodeEntropy = Buffer.from(seed, "hex")
    const mnemonic = entropyToMnemonic(nodeEntropy, wordlist)
    const nodeSeed = getNodeSeedFromEntropy(nodeEntropy)

    entropyOutput += `  ${index}: "${seed}", // ${nodeId}\n`
    mnemonicsOutput += `  ${index}: "${mnemonic}",\n`
    seedsOutput += `  ${index}: "${nodeSeed.toString("hex")}",\n`
    idsOutput += `  "${nodeId}",\n`
  }
  entropyOutput += ENTROPY_SECTION_FOOTER
  mnemonicsOutput += MNEMONIC_SECTION_FOOTER
  seedsOutput += SEED_SECTION_FOOTER
  idsOutput += ID_SECTION_FOOTER

  const completeOutput = `${OUTPUT_FILE_HEADER}${[
    entropyOutput,
    mnemonicsOutput,
    seedsOutput,
    idsOutput,
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

function processNodeId(nodeId: string) {
  const match = TARGET_PATTERN.exec(nodeId)
  if (!match) {
    throw new Error(`Invalid address: ${nodeId}`)
  }

  return { match, nodeId }
}

function countHyphensAndUnderscores(nodeId: string): number {
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
function isNewOrBetter(index: number, nodeId: string) {
  const existing = foundMap.get(index)
  if (!existing) return true

  const existingHyphenCount = countHyphensAndUnderscores(existing.nodeId)
  const hyphenCount = countHyphensAndUnderscores(nodeId)

  return hyphenCount < existingHyphenCount
}

console.info("Incorporating new vanity addresses...")

const rl = createInterface({
  input: process.stdin,
  output: process.stdout,
  terminal: false,
})

rl.on("line", (line) => {
  if (!line || line.startsWith("#")) return

  if (line[1] === "x") line = line.slice(2)

  const [seedHex, id] = line.split(": ")

  if (!seedHex || !id) return

  const seed = Buffer.from(seedHex, "hex")

  const { match, nodeId } = processNodeId(id)
  const index = Number.parseInt(match[1]!, 10) - 1
  const friendlyId = nodeIndexToFriendlyId(index)

  if (index > MAX_INDEX) {
    if (isLargestEver(index)) {
      assert(getNodeIdFromEntropy(seed) === nodeId)
      foundMap.set(index, { seed: seedHex, nodeId })
      console.info(`${friendlyId} => ${nodeId} (new record!)`)
      saveResults()
    } else {
      console.info(`${friendlyId} => ${nodeId} (skipped, too large)`)
    }
  } else if (isNewOrBetter(index, nodeId)) {
    assert(getNodeIdFromEntropy(seed) === nodeId)
    foundMap.set(index, { seed: seedHex, nodeId })
    console.info(`${friendlyId} => ${nodeId}`)
    saveResults()
  }
})
