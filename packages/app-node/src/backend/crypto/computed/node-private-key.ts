import { createComputed } from "@dassie/lib-reactive"

import { databaseConfigPlain } from "../../config/database-config"
import { parseEd25519PrivateKey } from "../../utils/pem"

export const nodePrivateKeySignal = () =>
  createComputed((sig) =>
    parseEd25519PrivateKey(sig.get(sig.use(databaseConfigPlain).tlsDassieKey))
  )
