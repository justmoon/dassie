import { createComputed } from "@dassie/lib-reactive"

import { environmentConfigSignal } from "../../config/environment-config"
import { parseEd25519PrivateKey } from "../../utils/pem"

export const nodePrivateKeySignal = () =>
  createComputed((sig) =>
    parseEd25519PrivateKey(sig.get(environmentConfigSignal).tlsDassieKey)
  )
