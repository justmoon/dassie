import { OerType } from "./base-type"
import { OctetStringOptions, octetString } from "./octet-string"
import { ParseError, SerializeError } from "./utils/errors"
import type { ParseContext, SerializeContext } from "./utils/parse"

type EncodingType = "utf8" | "ascii"

export const OerString = class extends OerType<string> {
  octetString: OerType<Uint8Array>
  textDecoder: TextDecoder
  textEncoder: TextEncoder

  constructor(
    octetStringOptions: OctetStringOptions | undefined,
    encoding: EncodingType,
    readonly filterArray?: boolean[]
  ) {
    super()

    this.octetString = octetString(octetStringOptions)
    this.textDecoder = new TextDecoder(encoding)
    this.textEncoder = new TextEncoder()
  }

  parseWithContext(context: ParseContext, offset: number) {
    const parseResult = this.octetString.parseWithContext(context, offset)

    if (parseResult instanceof ParseError) {
      return parseResult
    }

    const [octetStringValue, totalLength] = parseResult

    if (this.filterArray) {
      for (let index = 0; index < octetStringValue.length; index++) {
        // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
        if (!this.filterArray[octetStringValue[index]!]) {
          return new ParseError(
            // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
            `Invalid character 0x${octetStringValue[index]!.toString(
              16
            ).padStart(2, "0")} in string`,
            context.uint8Array,
            // We don't know in this context if it is a fixed length string or not or how long the length prefix is.
            // However, we do know where the failed character is in relation to the end of the string.
            offset + totalLength - octetStringValue.length + index
          )
        }
      }
    }

    return [this.textDecoder.decode(parseResult[0]), totalLength] as const
  }

  serializeWithContext(value: string) {
    const encodedValue = this.textEncoder.encode(value)
    const serializeResult = this.octetString.serializeWithContext(encodedValue)

    if (serializeResult instanceof SerializeError) {
      return serializeResult
    }

    return [
      (context: SerializeContext, offset: number) => {
        serializeResult[0](context, offset)
      },
      serializeResult[1],
    ] as const
  }
}

export const utf8String = (octetStringOptions?: OctetStringOptions) => {
  return new OerString(octetStringOptions, "utf8")
}

export const ia5String = (octetStringOptions?: OctetStringOptions) => {
  const filterArray = Array.from<boolean>({ length: 128 }).fill(true)
  return new OerString(octetStringOptions, "ascii", filterArray)
}

export const visibleString = (octetStringOptions?: OctetStringOptions) => {
  const filterArray = Array.from<boolean>({ length: 128 }).fill(
    true,
    0x20,
    0x7e
  )
  return new OerString(octetStringOptions, "ascii", filterArray)
}

export const numericString = (octetStringOptions?: OctetStringOptions) => {
  const filterArray = Array.from<boolean>({ length: 128 }).fill(
    true,
    0x30,
    0x39
  ) // 0-9
  filterArray[0x20] = true // SPACE
  return new OerString(octetStringOptions, "ascii", filterArray)
}

export const printableString = (octetStringOptions?: OctetStringOptions) => {
  const filterArray = Array.from<boolean>({ length: 128 })
    .fill(true, 0x41, 0x5a) // A-Z
    .fill(true, 0x61, 0x7a) // a-z
    .fill(true, 0x30, 0x39) // 0-9
  for (const character of " '()+,-./:=?") {
    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    filterArray[character.codePointAt(0)!] = true
  }

  return new OerString(octetStringOptions, "ascii", filterArray)
}
