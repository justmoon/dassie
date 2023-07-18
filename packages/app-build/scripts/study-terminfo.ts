#!/usr/bin/env -S npx vite-node
import { $ } from "execa"

import { mkdir, readFile, writeFile } from "node:fs/promises"
import { resolve } from "node:path"

const DIST_PATH = new URL("../dist", import.meta.url).pathname
const CACHE_FILE = resolve(DIST_PATH, "terminfo.csv")

const toeResult = await $`toe -as`.pipeStdout!($`awk '{print $1}'`)
const toeOutput = toeResult.stdout.split("\n").filter(Boolean)

const terminalColors = new Map<string, number>()

type CacheData = [terminalName: string, supportedColors: number][]
try {
  const rawCache = await readFile(CACHE_FILE, "utf8")
  const cache = rawCache
    .split("\n")
    .filter(Boolean)
    .map((line) => line.split(",")) as CacheData
  for (const [term, colors] of cache) {
    terminalColors.set(term, colors)
  }
  console.info(
    `Loaded color support information about ${terminalColors.size} terminals from cache`
  )
} catch {
  console.info(
    `Querying terminfo for color support for ${toeOutput.length} terminals`
  )
  console.info("This may take a while...")
  for (const term of toeOutput) {
    try {
      const tputResult = await $`tput -T${term} colors`
      const tputOutput = tputResult.stdout.trim()

      terminalColors.set(term, Number(tputOutput))
    } catch {
      // ignore errors
      console.warn(`Failed to query terminfo for ${term}`)
    }
  }

  await mkdir(DIST_PATH, { recursive: true })
  await writeFile(
    CACHE_FILE,
    [...terminalColors].map(([term, colors]) => `${term},${colors}`).join("\n")
  )
}

const TERM_ENVS = new Set([
  "Eterm",
  // "cons25",
  // "console",
  // "cygwin",
  // "dtterm",
  // "gnome",
  // "hurd",
  // "jfbterm",
  // "konsole",
  // "kterm",
  // "mlterm",
  // "mosh",
  // "putty",
  // "st",
  // "rxvt-unicode-24bit",
  // "terminator",
])

const TERM_ENVS_REG_EXP = [
  /16color/,
  /256color/,
  // /ansi/,
  /^linux/,
  // /^gnome/,
  // /^con\d*x\d/,
  // /^rxvt/,
  // /^screen/,
  // /^xterm/,
  // /^vt100/,
  // /^kitty/,
  // /^terminology/,
  // /^termite/,
  // /^alacritty/,
  // /^konsole/,
  /^vte/,
  // /-direct$/,
]

const predicate = (term: string) =>
  TERM_ENVS.has(term) || TERM_ENVS_REG_EXP.some((regExp) => regExp.test(term))

const truePositives = new Set<string>(),
  falsePositives = new Set<string>(),
  falseNegatives = new Set<string>(),
  trueNegatives = new Set<string>()

for (const [term, colors] of terminalColors) {
  const terminfoOpinion = colors > 2
  const predicateOpinion = predicate(term)

  if (terminfoOpinion && predicateOpinion) {
    truePositives.add(term)
  } else if (!terminfoOpinion && predicateOpinion) {
    falsePositives.add(term)
  } else if (terminfoOpinion && !predicateOpinion) {
    falseNegatives.add(term)
  } else {
    trueNegatives.add(term)
  }
}

console.info("truePositives:", truePositives.size)
console.info("trueNegatives:", trueNegatives.size)
console.info(
  "falsePositives:",
  falsePositives.size,
  [...falsePositives].join("\n")
)
console.info(
  "falseNegatives:",
  falseNegatives.size,
  [...falseNegatives]
    .map((term) => `${term}=${terminalColors.get(term) ?? ""}`)
    .join("\n")
)
