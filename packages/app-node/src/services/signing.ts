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

  sign = (data: string | Buffer) => {
    const message = Buffer.isBuffer(data) ? data : Buffer.from(data, "utf8")
    return Buffer.from(sign(this.privateKey, message))
  }
}
