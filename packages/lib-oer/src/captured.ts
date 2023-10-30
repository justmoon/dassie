import { OerType } from "./base-type"
import { ParseError } from "./utils/errors"
import type { ParseContext, SerializeContext } from "./utils/parse"

export class OerCaptured<TParseValue, TSerializeValue> extends OerType<
  { value: TParseValue; bytes: Uint8Array },
  { value: TSerializeValue } | { bytes: Uint8Array }
> {
  constructor(readonly subType: OerType<TParseValue, TSerializeValue>) {
    super()
  }

  clone() {
    return new OerCaptured(this.subType)
  }

  parseWithContext(context: ParseContext, offset: number) {
    const subTypeResult = this.subType.parseWithContext(context, offset)

    if (subTypeResult instanceof ParseError) {
      return subTypeResult
    }

    const [subTypeValue, subTypeLength] = subTypeResult

    return [
      {
        value: subTypeValue,
        bytes: new Uint8Array(
          context.uint8Array.buffer,
          context.uint8Array.byteOffset + offset,
          subTypeLength,
        ),
      },
      subTypeLength,
    ] as const
  }

  serializeWithContext(
    input: { value: TSerializeValue } | { bytes: Uint8Array },
  ) {
    if ("value" in input) {
      return this.subType.serializeWithContext(input.value)
    } else {
      const serializer = (context: SerializeContext, offset: number) => {
        context.uint8Array.set(input.bytes, offset)
      }
      serializer.size = input.bytes.length
      return serializer
    }
  }
}

export const captured = <TParseValue, TSerializeValue>(
  subType: OerType<TParseValue, TSerializeValue>,
) => {
  return new OerCaptured(subType)
}
