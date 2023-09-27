import { bytesToHex } from "@noble/hashes/utils"

import {
  SEED_PATH_NODE,
  SEED_PATH_NODE_LOGIN,
} from "../../common/constants/seed-paths"
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

export const logout = async () => {
  const response = await fetch("/api/logout", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
  })

  if (!response.ok) {
    throw new Error(`Logout failed with status ${response.status}`)
  }
}

export const setup = async (
  binarySeed: Uint8Array,
  setupAuthorizationToken: string
) => {
  const loginAuthorizationSignature = getPrivateSeedAtPath(
    binarySeed,
    SEED_PATH_NODE_LOGIN
  )

  const response = await fetch("/api/setup", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      setupAuthorizationToken,
      rawDassieKeyHex: bytesToHex(
        getPrivateSeedAtPath(binarySeed, SEED_PATH_NODE)
      ),
      loginAuthorizationSignature: bytesToHex(loginAuthorizationSignature),
    }),
  })

  if (!response.ok) {
    throw new Error(`Setup failed with status ${response.status}`)
  }
}
