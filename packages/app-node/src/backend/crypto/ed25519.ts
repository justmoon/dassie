import { etc } from "@noble/ed25519"

import { createHash } from "node:crypto"

// eslint-disable-next-line @dassie/no-top-level-side-effects -- Enable synchronous sha512 hashing feature
etc.sha512Sync = (...m) =>
  createHash("sha512")
    .update(etc.concatBytes(...m))
    .digest()

export { getPublicKey, signAsync } from "@noble/ed25519"
