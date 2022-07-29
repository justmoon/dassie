import {
  AnyOerType,
  Infer,
  InferSerialize,
  IntermediateSerializationResult,
  OerType,
} from "./base-type"
import { integerAsBigint } from "./integer-as-bigint"
import { ParseError, SerializeError } from "./utils/errors"
import type { ParseContext, SerializeContext } from "./utils/parse"

export const sequenceOf = <TShape extends AnyOerType>(
  sequenceOfShape: TShape
) => {
  const sizeOer = integerAsBigint([0n, undefined])

  const OerSequenceOf = class extends OerType<
    Infer<TShape>[],
    InferSerialize<TShape>[]
  > {
    parseWithContext(context: ParseContext, offset: number) {
      const sizeResult = sizeOer.parseWithContext(context, offset)

      if (sizeResult instanceof ParseError) {
        return sizeResult
      }

      const [size, sizeOfSize] = sizeResult

      const result: Infer<TShape>[] = []
      let totalLength = sizeOfSize
      for (let index = 0n; index < size; index++) {
        const elementResult = sequenceOfShape.parseWithContext(
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
      const sizeResult = sizeOer.serializeWithContext(BigInt(input.length))

      if (sizeResult instanceof SerializeError) {
        return sizeResult
      }

      const [sizeSerializer, sizeOfSize] = sizeResult

      let totalLength = sizeOfSize
      const serializers: IntermediateSerializationResult[] = Array.from({
        length: input.length,
      })
      for (const [index, element] of input.entries()) {
        const elementResult = sequenceOfShape.serializeWithContext(element)

        if (elementResult instanceof SerializeError) {
          return elementResult
        }

        totalLength += elementResult[1]
        serializers[index] = elementResult
      }

      return [
        (context: SerializeContext, offset: number) => {
          const sizeSerializeResult = sizeSerializer(context, offset)

          if (sizeSerializeResult instanceof SerializeError) {
            return sizeSerializeResult
          }

          let totalLength = sizeOfSize
          for (const [serializer, elementLength] of serializers) {
            const elementResult = serializer(context, offset + totalLength)
            if (elementResult instanceof SerializeError) {
              return elementResult
            }
            totalLength += elementLength
          }

          return
        },
        totalLength,
      ] as const
    }
  }

  return new OerSequenceOf()
}
