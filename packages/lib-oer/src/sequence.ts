import type { ConditionalExcept, ConditionalPick, Simplify } from "type-fest"

import {
  AnyOerType,
  IntermediateSerializationResult,
  OerConstant,
  OerOptional,
  OerType,
} from "./base-type"
import { ParseError, SerializeError } from "./utils/errors"
import type { ParseContext, SerializeContext } from "./utils/parse"

export type ObjectShape = Record<string, AnyOerType>

// Takes the type
export type InferObjectParseShape<TShape extends ObjectShape> = {
  [key in keyof TShape]: TShape[key] extends OerType<infer K, never> ? K : never
}

export type InferObjectSerializeShape<TShape extends ObjectShape> = {
  [key in keyof ConditionalExcept<
    // We use ConditionalExcept to remove any constants from the serialize shape
    TShape,
    OerConstant<unknown, unknown> | OerOptional<unknown, unknown>
  >]: TShape[key] extends OerType<unknown, infer K> ? K : never
} & {
  [key in keyof ConditionalPick<
    TShape,
    OerOptional<unknown, unknown>
  >]?: TShape[key] extends OerType<unknown, infer K> ? K : never
}

type InferOerSequenceParseShape<TConfig extends OerSequenceShape> = Simplify<
  InferObjectParseShape<TConfig["root"]> &
    InferObjectParseShape<TConfig["extensions"][string]>
>

type InferOerSequenceSerializeShape<TConfig extends OerSequenceShape> =
  Simplify<
    InferObjectSerializeShape<TConfig["root"]> &
      InferObjectSerializeShape<TConfig["extensions"][string]>
  >

export interface OerSequenceShape {
  root: ObjectShape
  isExtensible: boolean
  extensions: Record<string, ObjectShape>
}

export class OerSequence<TShape extends OerSequenceShape> extends OerType<
  InferOerSequenceParseShape<TShape>,
  InferOerSequenceSerializeShape<TShape>
> {
  private rootEntries: Map<string, AnyOerType>
  private isExtensible: boolean
  private extensions: Map<string, Map<string, AnyOerType>>

  private rootOptionalFields: string[]

  constructor(sequenceShape: TShape) {
    super()

    this.rootEntries = new Map(Object.entries(sequenceShape.root))

    this.extensions = new Map(
      Object.entries(sequenceShape.extensions).map(([key, value]) => [
        key,
        new Map(Object.entries(value)),
      ])
    )

    this.isExtensible = sequenceShape.isExtensible || this.extensions.size > 0

    this.rootOptionalFields = [...this.rootEntries]
      .filter(([, value]) => value instanceof OerOptional)
      .map((v) => v[0])
  }

  parseWithContext(context: ParseContext, offset: number) {
    const { uint8Array } = context

    // --- Preamble ---

    let preambleByteLength = 0
    const optionalFieldsPresence = new Map<string, boolean>()
    let extensionBit = false

    if (this.isExtensible || this.rootOptionalFields.length > 0) {
      const firstByte = uint8Array[offset]
      if (firstByte == undefined) {
        return new ParseError(
          "unable to read sequence preamble - end of buffer",
          uint8Array,
          offset
        )
      }
      extensionBit = Boolean(firstByte & 0b1000_0000)
      const preambleBitLength =
        (this.isExtensible ? 1 : 0) + this.rootOptionalFields.length
      preambleByteLength = Math.ceil(preambleBitLength / 8)

      if (uint8Array.length < offset + preambleByteLength) {
        return new ParseError(
          `unable to read sequence preamble - end of buffer`,
          uint8Array,
          uint8Array.length
        )
      }

      for (const [
        index,
        optionalFieldName,
      ] of this.rootOptionalFields.entries()) {
        const bitIndex = index + (this.isExtensible ? 1 : 0)
        if (
          (uint8Array[offset + Math.floor(bitIndex / 8)]! >>
            (7 - (bitIndex % 8))) &
          1
        ) {
          optionalFieldsPresence.set(optionalFieldName, true)
        }
      }

      if (
        uint8Array[offset + preambleByteLength - 1]! <<
          (32 - 8 + (preambleBitLength % 8)) !==
        0
      ) {
        return new ParseError(
          "invalid sequence preamble - unused bits must be zero",
          uint8Array,
          offset + preambleByteLength - 1
        )
      }
    }

    // --- Root ---

    const resultObject: Record<string, unknown> = {}
    let totalLength = 0
    for (const [key, oer] of this.rootEntries) {
      if (oer instanceof OerOptional && !optionalFieldsPresence.get(key)) {
        // If there is no default, oer._default will be undefined, which just so happens to be the value that we want to return in that case
        resultObject[key] = oer.defaultValue
        continue
      }

      const result = oer.parseWithContext(
        context,
        offset + preambleByteLength + totalLength
      )
      if (result instanceof ParseError) {
        return result
      }
      resultObject[key] = result[0]
      totalLength += result[1]
    }

    // --- Extensions ---

    // TODO

    return [
      resultObject as InferOerSequenceParseShape<TShape>,
      preambleByteLength + totalLength,
    ] as const
  }

  serializeWithContext(input: InferOerSequenceSerializeShape<TShape>) {
    let totalLength = 0

    // --- Preamble ---
    const preambleLength =
      this.isExtensible || this.rootOptionalFields.length > 0
        ? Math.ceil((1 + this.rootOptionalFields.length) / 8)
        : 0
    totalLength += preambleLength

    // --- Root ---

    const serializers: (IntermediateSerializationResult | undefined)[] =
      Array.from({
        length: this.rootEntries.size,
      })
    let index = 0
    for (const [key, oer] of this.rootEntries) {
      if (oer instanceof OerOptional && !(key in input)) {
        continue
      }

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
        const { uint8Array } = context

        let cursor = 0

        // --- Preamble ---

        if (this.isExtensible || this.rootOptionalFields.length > 0) {
          for (const [
            index,
            optionalFieldName,
          ] of this.rootOptionalFields.entries()) {
            if (optionalFieldName in input) {
              const bitIndex = index + (this.isExtensible ? 1 : 0)
              uint8Array[offset + Math.floor(bitIndex / 8)]! |=
                1 << (7 - (bitIndex % 8))
            }
          }
          cursor += preambleLength
        }

        // --- Root ---

        for (const serializer of serializers) {
          if (!serializer) continue

          serializer[0](context, offset + cursor)
          cursor += serializer[1]
        }
      },
      totalLength,
    ] as const
  }

  extensible() {
    this.isExtensible = true
    return this
  }
}

export const sequence = <TRootShape extends ObjectShape>(
  sequenceShape: TRootShape
) => {
  return new OerSequence({
    root: sequenceShape,
    isExtensible: false,
    extensions: {},
  })
}
