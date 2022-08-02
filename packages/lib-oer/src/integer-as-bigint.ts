import { OerType } from "./base-type"
import {
  isUnsignedBigint,
  signedBigintByteLength,
  unsignedBigintByteLength,
} from "./utils/bigint"
import { ParseError, SerializeError } from "./utils/errors"
import {
  parseLengthPrefix,
  predictLengthPrefixLength,
  serializeLengthPrefix,
} from "./utils/length-prefix"
import type { ParseContext, SerializeContext } from "./utils/parse"
import { Range, parseRange } from "./utils/range"

interface IntegerOptions {
  minimumValue?: bigint
  maximumValue?: bigint
}

export interface FixedIntegerOptions extends IntegerOptions {
  minimumValue: bigint
  maximumValue: bigint
}

export const UINT_MIN = 0n
export const UINT8_MAX = 255n
export const UINT16_MAX = 65_535n
export const UINT32_MAX = 4_294_967_295n
export const UINT64_MAX = 18_446_744_073_709_551_615n

export const INT8_MIN = -128n
export const INT8_MAX = 127n
export const INT16_MIN = -32_768n
export const INT16_MAX = 32_767n
export const INT32_MIN = -2_147_483_648n
export const INT32_MAX = 2_147_483_647n
export const INT64_MIN = -9_223_372_036_854_775_808n
export const INT64_MAX = 9_223_372_036_854_775_807n

export abstract class OerFixedIntegerBigint extends OerType<bigint> {
  abstract readonly type: "Uint" | "Int"
  abstract readonly size: 8 | 16 | 32 | 64

  constructor(readonly options: FixedIntegerOptions) {
    super()
  }

  parseWithContext({ uint8Array, dataView }: ParseContext, offset: number) {
    if (offset + this.size / 8 > dataView.byteLength) {
      return new ParseError(
        `unable to read fixed length integer of size ${
          this.size / 8
        } bytes - end of buffer`,
        uint8Array,
        uint8Array.byteLength
      )
    }

    const value =
      this.size === 64
        ? dataView[`getBig${this.type}${this.size}`](offset)
        : BigInt(dataView[`get${this.type}${this.size}`](offset))

    if (value < this.options.minimumValue) {
      return new ParseError(
        `unable to read fixed length integer of size ${
          this.size / 8
        } bytes - value ${value} is less than minimum value ${
          this.options.minimumValue
        }`,
        uint8Array,
        offset
      )
    }

    if (value > this.options.maximumValue) {
      return new ParseError(
        `unable to read fixed length integer of size ${
          this.size / 8
        } bytes - value ${value} is greater than maximum value ${
          this.options.maximumValue
        }`,
        uint8Array,
        offset
      )
    }

    return [value, this.size / 8] as const
  }

  serializeWithContext(value: bigint) {
    const serializer = (context: SerializeContext, offset: number) => {
      if (value < this.options.minimumValue) {
        return new SerializeError(
          `integer must be >= ${this.options.minimumValue}`
        )
      }

      if (value > this.options.maximumValue) {
        return new SerializeError(
          `integer must be <= ${this.options.maximumValue}`
        )
      }

      if (this.size === 64) {
        context.dataView[`setBig${this.type}${this.size}`](offset, value)
      } else {
        context.dataView[`set${this.type}${this.size}`](offset, Number(value))
      }

      return
    }
    serializer.size = this.size / 8
    return serializer
  }
}

export class OerUint8Bigint extends OerFixedIntegerBigint {
  readonly type = "Uint"
  readonly size = 8
}
export class OerUint16Bigint extends OerFixedIntegerBigint {
  readonly type = "Uint"
  readonly size = 16
}
export class OerUint32Bigint extends OerFixedIntegerBigint {
  readonly type = "Uint"
  readonly size = 32
}
export class OerUint64Bigint extends OerFixedIntegerBigint {
  readonly type = "Uint"
  readonly size = 64
}

export class OerInt8Bigint extends OerFixedIntegerBigint {
  readonly type = "Int"
  readonly size = 8
}
export class OerInt16Bigint extends OerFixedIntegerBigint {
  readonly type = "Int"
  readonly size = 16
}
export class OerInt32Bigint extends OerFixedIntegerBigint {
  readonly type = "Int"
  readonly size = 32
}
export class OerInt64Bigint extends OerFixedIntegerBigint {
  readonly type = "Int"
  readonly size = 64
}

export class OerVariableUnsignedInteger extends OerType<bigint> {
  parseWithContext(context: ParseContext, offset: number) {
    const { uint8Array, dataView } = context
    const result = parseLengthPrefix(context, offset)

    if (result instanceof ParseError) {
      return result
    }

    const [length, lengthOfLength] = result

    if (length === 0) {
      return new ParseError(
        `unable to read variable length integer - length must not be 0`,
        uint8Array,
        offset
      )
    }

    offset += lengthOfLength

    let value = 0n
    let index = 0
    while (index < length) {
      if (index + 4 < length) {
        value = (value << 32n) + BigInt(dataView.getUint32(offset + index))
        index += 4
      } else {
        // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
        value = (value << 8n) + BigInt(uint8Array[offset + index]!)
        index += 1
      }
    }

    return [value, lengthOfLength + length] as const
  }

  serializeWithContext(input: bigint) {
    if (!isUnsignedBigint(input)) {
      return new SerializeError(
        `expected unsigned bigint, got ${typeof input}: ${input}`
      )
    }

    const length = unsignedBigintByteLength(input)
    const lengthOfLengthPrefix = predictLengthPrefixLength(length)

    if (lengthOfLengthPrefix instanceof SerializeError) {
      return lengthOfLengthPrefix
    }

    const serializer = ({ uint8Array }: SerializeContext, offset: number) => {
      const length = unsignedBigintByteLength(input)
      const lengthOfLengthPrefix = serializeLengthPrefix(
        length,
        uint8Array,
        offset
      )

      if (lengthOfLengthPrefix instanceof SerializeError) {
        return lengthOfLengthPrefix
      }

      let remainder: bigint = input

      for (let index = 0; index < length; index++) {
        const byte = remainder & 0xffn
        uint8Array[offset + lengthOfLengthPrefix + length - index - 1] =
          Number(byte)
        remainder >>= 8n
      }

      return
    }
    serializer.size = lengthOfLengthPrefix + length
    return serializer
  }
}

export class OerVariableSignedInteger extends OerType<bigint> {
  parseWithContext(context: ParseContext, offset: number) {
    const { uint8Array } = context
    const result = parseLengthPrefix(context, offset)

    if (result instanceof ParseError) {
      return result
    }

    const [length, lengthOfLength] = result

    if (length === 0) {
      return new ParseError(
        `unable to read variable length integer - length must not be 0`,
        uint8Array,
        offset
      )
    }

    offset += lengthOfLength

    let value = 0n
    let index = 0
    while (index < length) {
      value = (value << 8n) | BigInt(uint8Array[offset + index]!)
      index += 1
    }

    const twosComplement = 2n ** (BigInt(length) * 8n)
    if (value >= twosComplement >> 1n) {
      value = -(twosComplement - value)
    }

    return [value, lengthOfLength + length] as const
  }

  serializeWithContext(input: bigint) {
    const length = signedBigintByteLength(input)
    const lengthOfLengthPrefix = predictLengthPrefixLength(length)

    if (lengthOfLengthPrefix instanceof SerializeError) {
      return lengthOfLengthPrefix
    }

    const serializer = ({ uint8Array }: SerializeContext, offset: number) => {
      const lengthPrefixSerializeResult = serializeLengthPrefix(
        length,
        uint8Array,
        offset
      )

      if (lengthPrefixSerializeResult instanceof SerializeError) {
        return lengthPrefixSerializeResult
      }

      // Calculate two's complement if input is negative
      if (input < 0n) {
        input += 2n ** BigInt(length * 8)
      }

      let remainder: bigint = input

      for (let index = 0; index < length; index++) {
        const byte = remainder & 0xffn
        uint8Array[offset + lengthOfLengthPrefix + length - index - 1] =
          Number(byte)
        remainder >>= 8n
      }

      return
    }
    serializer.size = lengthOfLengthPrefix + length
    return serializer
  }
}

export const integerAsBigint = (range?: Range<bigint>) => {
  const [minimumValue, maximumValue] = parseRange(range)

  if (minimumValue != undefined && maximumValue != undefined) {
    const fixedOptions = {
      minimumValue,
      maximumValue,
    }

    // Fixed size integer encodings
    if (minimumValue >= UINT_MIN) {
      if (maximumValue <= UINT8_MAX) {
        return new OerUint8Bigint(fixedOptions)
      } else if (maximumValue <= UINT16_MAX) {
        return new OerUint16Bigint(fixedOptions)
      } else if (maximumValue <= UINT32_MAX) {
        return new OerUint32Bigint(fixedOptions)
      } else if (maximumValue <= UINT64_MAX) {
        return new OerUint64Bigint(fixedOptions)
      }
    } else {
      if (minimumValue <= INT8_MIN && maximumValue <= INT8_MAX) {
        return new OerInt8Bigint(fixedOptions)
      } else if (minimumValue <= INT16_MIN && maximumValue <= INT16_MAX) {
        return new OerInt16Bigint(fixedOptions)
      } else if (minimumValue <= INT32_MIN && maximumValue <= INT32_MAX) {
        return new OerInt32Bigint(fixedOptions)
      } else if (minimumValue <= INT64_MIN && maximumValue <= INT64_MAX) {
        return new OerInt64Bigint(fixedOptions)
      }
    }
  }

  // Variable size integer encodings
  return minimumValue != undefined && minimumValue >= UINT_MIN
    ? new OerVariableUnsignedInteger()
    : new OerVariableSignedInteger()
}

export const uint8Bigint = () => integerAsBigint([UINT_MIN, UINT8_MAX])
export const uint16Bigint = () => integerAsBigint([UINT_MIN, UINT16_MAX])
export const uint32Bigint = () => integerAsBigint([UINT_MIN, UINT32_MAX])
export const uint64Bigint = () => integerAsBigint([UINT_MIN, UINT64_MAX])

export const int8Bigint = () => integerAsBigint([INT8_MIN, INT8_MAX])
export const int16Bigint = () => integerAsBigint([INT16_MIN, INT16_MAX])
export const int32Bigint = () => integerAsBigint([INT32_MIN, INT32_MAX])
export const int64Bigint = () => integerAsBigint([INT64_MIN, INT64_MAX])
