import type {
  ConditionalExcept,
  OptionalKeysOf as OptionalKeys,
  RequiredKeysOf as RequiredKeys,
  Simplify,
} from "type-fest"

import {
  AnyOerType,
  IntermediateSerializationResult,
  OerConstant,
  OerType,
} from "./base-type"
import { ParseError, SerializeError } from "./utils/errors"
import type { ParseContext, SerializeContext } from "./utils/parse"

export type ObjectShape = Record<string, AnyOerType>

// Takes the type
export type InferObjectParseShape<TShape extends ObjectShape> = {
  [key in OptionalKeys<TShape>]?: TShape[key] extends OerType<infer K, never>
    ? K
    : never
} & {
  [key in RequiredKeys<TShape>]: TShape[key] extends OerType<infer K, never>
    ? K
    : never
}

export type InferObjectSerializeShape<TShape extends ObjectShape> = {
  [key in OptionalKeys<
    // We use ConditionalExcept to remove any constants from the serialize shape
    ConditionalExcept<TShape, OerConstant<unknown, unknown>>
  >]?: TShape[key] extends OerType<unknown, infer K> ? K : never
} & {
  [key in RequiredKeys<
    // We use ConditionalExcept to remove any constants from the serialize shape
    ConditionalExcept<TShape, OerConstant<unknown, unknown>>
  >]: TShape[key] extends OerType<unknown, infer K> ? K : never
}

export const sequence = <TShape extends ObjectShape>(sequenceShape: TShape) => {
  const shapeEntries = Object.entries(sequenceShape)

  const OerSequence = class extends OerType<
    Simplify<InferObjectParseShape<TShape>>,
    Simplify<InferObjectSerializeShape<TShape>>
  > {
    parseWithContext(context: ParseContext, offset: number) {
      const resultObject: Record<string, unknown> = {}
      let totalLength = 0
      for (const [key, oer] of shapeEntries) {
        const result = oer.parseWithContext(context, offset + totalLength)
        if (result instanceof ParseError) {
          return result
        }
        resultObject[key] = result[0]
        totalLength += result[1]
      }

      return [
        resultObject as InferObjectParseShape<TShape>,
        totalLength,
      ] as const
    }

    serializeWithContext(input: InferObjectSerializeShape<TShape>) {
      let totalLength = 0
      const serializers: IntermediateSerializationResult[] = Array.from({
        length: shapeEntries.length,
      })
      let index = 0
      for (const [key, oer] of shapeEntries) {
        const result = oer.serializeWithContext(
          (input as Record<string, unknown>)[key]
        )

        if (result instanceof SerializeError) {
          return result
        }

        totalLength += result[1]
        serializers[index++] = result
      }

      return [
        (context: SerializeContext, offset: number) => {
          let cursor = 0
          for (const serializer of serializers) {
            serializer[0](context, offset + cursor)
            cursor += serializer[1]
          }
        },
        totalLength,
      ] as const
    }
  }

  return new OerSequence()
}
