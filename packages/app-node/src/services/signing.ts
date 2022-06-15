import { createPrivateKey, createPublicKey, createSign } from "node:crypto"
import type { KeyObject } from "node:crypto"

import type { Config } from "../config"

export interface SigningContext {
  config: Config
}

export default class SigningService {
  private readonly privateKey: KeyObject
  readonly publicKey: KeyObject

  constructor(readonly context: SigningContext) {
    this.context = context

    this.privateKey = createPrivateKey(context.config.tlsXenKey)
    this.publicKey = createPublicKey(this.privateKey)
  }

  sign = (data: string | Buffer) => {
    const signer = createSign("sha512")
    signer.update(data)
    return signer.sign(this.privateKey)
  }
}
