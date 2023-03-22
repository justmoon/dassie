import { hmac } from "@noble/hashes/hmac"
import { sha256 } from "@noble/hashes/sha256"
import { hexToBytes } from "@noble/hashes/utils"

import { createActor } from "@dassie/lib-reactive"

import { walletStore } from "../stores/wallet"

export const signerService = () =>
  createActor((sig) => {
    const seed = sig.get(walletStore, (state) => state.seed)

    if (!seed) {
      return undefined
    }

    return {
      /**
       * Derives a private value from the global, private seed using a given path.
       *
       * The value is derived by iteratively calculating HMAC-SHA256(seed, segment) for each segment in the path, delimited by the forward slash ('/') character.
       *
       * @param path - A hierarchical identifier for the requested key.
       */
      getPrivateSeedAtPath: (path: string) => {
        const pathSegments = path.split("/")

        let key = hexToBytes(seed)
        for (const segment of pathSegments) {
          key = hmac(sha256, key, segment)
        }
      },
    }
  })
