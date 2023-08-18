import { hmac } from "@noble/hashes/hmac"
import { sha256 } from "@noble/hashes/sha256"

import { SeedPath } from "../../common/constants/seed-paths"

export const getPrivateSeedAtPath = (
  binarySeed: Uint8Array,
  path: SeedPath
) => {
  const pathSegments = path.split("/")

  let key = binarySeed
  for (const segment of pathSegments) {
    key = hmac(sha256, key, segment)
  }

  return key
}
