import { createComputed } from "@dassie/lib-reactive"

import { configSignal } from "../../config"
import { parseEd25519PrivateKey } from "../../utils/pem"

export const nodePrivateKeySignal = () =>
  createComputed((sig) =>
    parseEd25519PrivateKey(sig.get(configSignal).tlsDassieKey)
  )
