import { hmac } from "@noble/hashes/hmac"
import { sha256 } from "@noble/hashes/sha256"
import { hexToBytes } from "@noble/hashes/utils"

export const getPrivateSeedAtPath = (seed: string, path: string) => {
  const pathSegments = path.split("/")

  let key = hexToBytes(seed)
  for (const segment of pathSegments) {
    key = hmac(sha256, key, segment)
  }
}
