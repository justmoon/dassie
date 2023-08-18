import { bytesToHex } from "@noble/hashes/utils"

import { SEED_PATH_NODE_LOGIN } from "../../common/constants/seed-paths"
import { getPrivateSeedAtPath } from "./signer"

export const login = async (binarySeed: Uint8Array) => {
  const loginAuthorizationSignature = getPrivateSeedAtPath(
    binarySeed,
    SEED_PATH_NODE_LOGIN
  )

  const response = await fetch("/api/login", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      loginAuthorizationSignature: bytesToHex(loginAuthorizationSignature),
    }),
  })

  if (!response.ok) {
    throw new Error(`Login failed with status ${response.status}`)
  }
}
