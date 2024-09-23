import { Duplex, type DuplexOptions } from "node:stream"

const kCallback = Symbol("Callback")
const kOtherSide = Symbol("Other")

class DuplexSocket extends Duplex {
  [kCallback]: (() => void) | undefined;
  [kOtherSide]: DuplexSocket | undefined

  override _read() {
    const callback = this[kCallback]
    if (callback) {
      this[kCallback] = undefined
      callback()
    }
  }

  override _write(
    chunk: string | Buffer,
    _encoding: string,
    callback: () => void,
  ) {
    if (!this[kOtherSide]) {
      throw new Error("Not initialized")
    }

    this[kOtherSide][kCallback] = callback
    this[kOtherSide].push(chunk)
  }

  override _final(callback: () => void) {
    if (!this[kOtherSide]) {
      throw new Error("Not initialized")
    }

    this[kOtherSide].on("end", callback)
    this[kOtherSide].push(null)
  }
}

export class DuplexPair {
  public readonly socket1: DuplexSocket
  public readonly socket2: DuplexSocket

  constructor(options?: DuplexOptions) {
    this.socket1 = new DuplexSocket(options)
    this.socket2 = new DuplexSocket(options)
    this.socket1[kOtherSide] = this.socket2
    this.socket2[kOtherSide] = this.socket1
  }
}
