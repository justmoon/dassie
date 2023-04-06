import { etc } from "@noble/ed25519"

import { createHash } from "node:crypto"

etc.sha512Sync = (...m) =>
  createHash("sha512")
    .update(etc.concatBytes(...m))
    .digest()

export { getPublicKey, signAsync } from "@noble/ed25519"
