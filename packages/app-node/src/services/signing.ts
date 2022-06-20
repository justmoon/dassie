import { sign } from "@xen-ilp/lib-crypto"

import type { Config } from "../config"
import { parseEd25519PrivateKey } from "../utils/pem"

export interface SigningContext {
  config: Config
}

export default class SigningService {
  private readonly privateKey: Buffer

  constructor(readonly context: SigningContext) {
    this.context = context

    this.privateKey = parseEd25519PrivateKey(context.config.tlsXenKey)
  }

  sign = (data: Buffer) => {
    return Buffer.from(sign(this.privateKey, data))
  }
}
