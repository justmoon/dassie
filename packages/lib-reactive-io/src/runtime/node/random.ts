import { randomBytes } from "node:crypto"

import type { Random } from "@dassie/lib-reactive"

export class NodeRandomImplementation implements Random {
  randomBytes(length: number): Uint8Array {
    const buffer = randomBytes(length)
    return new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength)
  }
}
