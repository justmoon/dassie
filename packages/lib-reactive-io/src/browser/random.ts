import type { Random } from "@dassie/lib-reactive"

export class BrowserRandomImplementation implements Random {
  randomBytes(length: number): Uint8Array {
    // eslint-disable-next-line n/no-unsupported-features/node-builtins
    return crypto.getRandomValues(new Uint8Array(length))
  }
}

export function createRandom(): Random {
  return new BrowserRandomImplementation()
}
