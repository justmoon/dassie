import { readFileSync } from "node:fs"

const binaryDependenciesText = readFileSync(
  new URL("../../resources/binaries/binary-dependencies.txt", import.meta.url),
  "utf8",
)

const binaryDependencies = binaryDependenciesText
  .split("\n")
  .map((line) => line.split(" "))
  .filter((line) => line[0]!.startsWith("https://"))

export const VERIFIED_HASHES: Record<string, string> = Object.fromEntries(
  binaryDependencies.map(([url, hash]) => [url!, hash!]),
)
