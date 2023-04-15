import {
  type AnyOerType,
  type Infer,
  type InferSerialize,
  OerType,
  type Serializer,
} from "./base-type"
import { integerAsBigint } from "./integer-as-bigint"
import { ParseError, SerializeError } from "./utils/errors"
import type { ParseContext, SerializeContext } from "./utils/parse"

export class OerSequenceOf<TShape extends AnyOerType> extends OerType<
  Infer<TShape>[],
  InferSerialize<TShape>[]
> {
  private sizeOer: OerType<bigint>

  constructor(readonly sequenceOfShape: TShape) {
    super()

    this.sizeOer = integerAsBigint([0n, undefined])
  }

  clone() {
    return new OerSequenceOf(this.sequenceOfShape)
  }

  parseWithContext(context: ParseContext, offset: number) {
    const sizeResult = this.sizeOer.parseWithContext(context, offset)

    if (sizeResult instanceof ParseError) {
      return sizeResult
    }

    const [size, sizeOfSize] = sizeResult

    const result: Infer<TShape>[] = []
    let totalLength = sizeOfSize
    for (let index = 0n; index < size; index++) {
      const elementResult = this.sequenceOfShape.parseWithContext(
        context,
        offset + totalLength
      )
      if (elementResult instanceof ParseError) {
        return elementResult
      }
      const [element, elementLength] = elementResult
      result.push(element as Infer<TShape>)
      totalLength += elementLength
    }

    return [result, totalLength] as const
  }

  serializeWithContext(input: InferSerialize<TShape>[]) {
    const sizeSerializer = this.sizeOer.serializeWithContext(
      BigInt(input.length)
    )

    if (sizeSerializer instanceof SerializeError) {
      return sizeSerializer
    }

    let totalLength = sizeSerializer.size
    const serializers: Serializer[] = Array.from({
      length: input.length,
    })
    for (const [index, element] of input.entries()) {
      const elementSerializer =
        this.sequenceOfShape.serializeWithContext(element)

      if (elementSerializer instanceof SerializeError) {
        return elementSerializer
      }

      totalLength += elementSerializer.size
      serializers[index] = elementSerializer
    }

    const serializer = (context: SerializeContext, offset: number) => {
      const sizeSerializeResult = sizeSerializer(context, offset)

      if (sizeSerializeResult instanceof SerializeError) {
        return sizeSerializeResult
      }

      let totalLength = sizeSerializer.size
      for (const serializer of serializers) {
        const elementResult = serializer(context, offset + totalLength)
        if (elementResult instanceof SerializeError) {
          return elementResult
        }
        totalLength += serializer.size
      }

      return
    }

    serializer.size = totalLength
    return serializer
  }
}

export const sequenceOf = <TShape extends AnyOerType>(
  sequenceOfShape: TShape
) => {
  return new OerSequenceOf(sequenceOfShape)
}
