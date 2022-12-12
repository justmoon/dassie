import { OerType } from "./base-type"
import { octetString } from "./octet-string"
import { Alphabet, alphabetToFilterArray, printable } from "./utils/alphabet"
import { ParseError, SerializeError } from './utils/errors'
import type { ParseContext } from "./utils/parse"
import { NormalizedRange, Range, parseRange } from "./utils/range"

export type EncodingType = "utf8" | "ascii"

const convertCharacterRangeToOctetStringRange = (
  characterRange: NormalizedRange<number>,
  encoding: EncodingType
): NormalizedRange<number> =>
  encoding === "utf8"
    ? [
        characterRange[0],
        characterRange[1] != undefined ? characterRange[1] * 4 : undefined,
      ]
    : characterRange

export class OerString extends OerType<string> {
  private octetString: OerType<Uint8Array>
  private textDecoder: TextDecoder
  private textEncoder: TextEncoder

  constructor(
    readonly length: NormalizedRange<number>,
    readonly encoding: EncodingType,
    readonly filterArray?: boolean[]
  ) {
    super()

    this.octetString = octetString(
      convertCharacterRangeToOctetStringRange(length, encoding)
    )
    this.textDecoder = new TextDecoder(encoding)
    this.textEncoder = new TextEncoder()
  }

  clone() {
    return new OerString(this.length, this.encoding, this.filterArray)
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

    const decodedString = this.textDecoder.decode(parseResult[0])

    if (this.length[0] != undefined && decodedString.length < this.length[0]) {
      return new ParseError(
        `String is too short, expected at least ${this.length[0]} characters, got ${decodedString.length}`,
        context.uint8Array,
        offset
      )
    }

    if (this.length[1] != undefined && decodedString.length > this.length[1]) {
      return new ParseError(
        `String is too long, expected at most ${this.length[1]} characters, got ${decodedString.length}`,
        context.uint8Array,
        offset
      )
    }

    return [decodedString, totalLength] as const
  }

  serializeWithContext(value: string) {
    if (this.length[0] != undefined && value.length < this.length[0]) {
      return new SerializeError(
          `String is too short, expected at least ${this.length[0]} characters, got ${value.length}`,
      )
    }

    if (this.length[1] != undefined && value.length > this.length[1]) {
      return new SerializeError(
          `String is too long, expected at most ${this.length[1]} characters, got ${value.length}`,
      )
      }
    const encodedValue: Uint8Array = this.textEncoder.encode(value)
    return this.octetString.serializeWithContext(encodedValue)
  }

  from(characters: Alphabet) {
    return new OerString(
      this.length,
      this.encoding,
      alphabetToFilterArray(characters)
    )
  }
}

export const utf8String = (length?: Range<number>) => {
  return new OerString(parseRange(length), "utf8")
}

export const ia5String = (length?: Range<number>) => {
  const filterArray = Array.from<boolean>({ length: 128 }).fill(true)
  return new OerString(parseRange(length), "ascii", filterArray)
}

export const visibleString = (length?: Range<number>) => {
  const filterArray = Array.from<boolean>({ length: 128 }).fill(
    true,
    0x20,
    0x7e
  )
  return new OerString(parseRange(length), "ascii", filterArray)
}

export const numericString = (length?: Range<number>) => {
  const filterArray = Array.from<boolean>({ length: 128 }).fill(
    true,
    0x30,
    0x39
  ) // 0-9
  filterArray[0x20] = true // SPACE
  return new OerString(parseRange(length), "ascii", filterArray)
}

export const printableString = (length?: Range<number>) => {
  return new OerString(
    parseRange(length),
    "ascii",
    alphabetToFilterArray(printable)
  )
}
