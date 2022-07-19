export class ParseError extends Error {
  constructor(
    message: string,
    readonly data: Uint8Array,
    readonly offset: number
  ) {
    super(message)

    this.name = "ParseError"
  }
}

export class SerializeError extends Error {
  constructor(message: string) {
    super(message)

    this.name = "SerializeError"
  }
}
