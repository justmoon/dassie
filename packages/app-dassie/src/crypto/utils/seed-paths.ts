import { createHmac } from "node:crypto"

import { SEED_PATH_NODE, type SeedPath } from "../../constants/seed-paths"

export const getPrivateSeedAtPath = (
  nodePrivateKey: Uint8Array,
  path: SeedPath,
): Buffer => {
  if (path === SEED_PATH_NODE) {
    return Buffer.from(nodePrivateKey)
  }

  if (!path.startsWith(SEED_PATH_NODE + "/")) {
    throw new Error(
      `Unable to calculate seed values server-side unless their path starts with ${SEED_PATH_NODE}/: ${path}`,
    )
  }

  return calculatePathHmac(
    Buffer.from(nodePrivateKey),
    path.slice(SEED_PATH_NODE.length + 1),
  )
}

export const calculatePathHmac = (key: Buffer, path: string): Buffer => {
  const pathSegments = path.split("/")

  for (const segment of pathSegments) {
    const hmac = createHmac("sha256", key)
    key = hmac.update(segment).digest()
  }

  return key
}
