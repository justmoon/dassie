import { randomBytes } from "node:crypto"

import type { Random } from "@dassie/lib-reactive"
import { bufferToUint8Array } from "@dassie/lib-type-utils"

export class NodeRandomImplementation implements Random {
  randomBytes(length: number): Uint8Array {
    return bufferToUint8Array(randomBytes(length))
  }
}
