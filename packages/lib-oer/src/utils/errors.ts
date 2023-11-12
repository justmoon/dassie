import { ParseFailure, SerializeFailure } from "./failures"

export class ParseError extends Error {
  readonly data: Uint8Array
  readonly offset: number

  constructor(cause: ParseFailure) {
    super(cause.message)

    this.name = "ParseError"
    this.data = cause.data
    this.offset = cause.offset
  }
}

export class SerializeError extends Error {
  constructor(cause: SerializeFailure) {
    super(cause.message)

    this.name = "SerializeError"
  }
}
