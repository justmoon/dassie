import { hmac } from "@noble/hashes/hmac"
import { sha256 } from "@noble/hashes/sha256"

export const getPrivateSeedAtPath = (binarySeed: Uint8Array, path: string) => {
  const pathSegments = path.split("/")

  let key = binarySeed
  for (const segment of pathSegments) {
    key = hmac(sha256, key, segment)
  }

  return key
}
