import { Failure } from "@dassie/lib-type-utils"

import { uint8ArrayToHex } from "./hex"

const PARSE_ERROR_CONTEXT_BYTES = 20

export class ParseFailure extends Failure {
  readonly name = "ParseFailure"
  readonly message: string

  constructor(
    message: string,
    readonly data: Uint8Array,
    readonly offset: number,
  ) {
    const hexExcerptRange = [
      Math.max(0, offset - PARSE_ERROR_CONTEXT_BYTES),
      Math.min(data.length, offset + PARSE_ERROR_CONTEXT_BYTES),
    ] as const

    const hexExcerpt = uint8ArrayToHex(
      data.slice(hexExcerptRange[0], hexExcerptRange[1]),
    )

    const formattedMessage = `${message}\n\n  ${
      hexExcerptRange[0] === 0 ? " " : "…"
    } ${hexExcerpt} ${
      hexExcerptRange[1] === data.length ? " " : "…"
    }\n    ${"".padEnd((offset - hexExcerptRange[0]) * 3, " ")}^^`

    super()

    this.message = formattedMessage
  }
}

export class SerializeFailure extends Failure {
  readonly name = "SerializeFailure"

  constructor(readonly message: string) {
    super()
  }
}
