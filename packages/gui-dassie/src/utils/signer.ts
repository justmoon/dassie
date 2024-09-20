import { hmac } from "@noble/hashes/hmac"
import { sha256 } from "@noble/hashes/sha256"

import type { SeedPath } from "@dassie/app-dassie/src/constants/seed-paths"

export const getPrivateSeedAtPath = (
  binarySeed: Uint8Array,
  path: SeedPath,
) => {
  const pathSegments = path.split("/")

  let key = binarySeed
  for (const segment of pathSegments) {
    key = hmac(sha256, key, segment)
  }

  return key
}
