import type { ConditionalExcept, ConditionalPick } from "type-fest"

import {
  type AnyOerType,
  type Infer,
  type InferSerialize,
  OerConstant,
  OerOptional,
  OerType,
  type Serializer,
} from "./base-type"
import type {
  AnyObjectSetField,
  InformationObjectSetStateMap,
} from "./information-object/object-set"
import type {
  InferInformationObjectParseShape,
  InferInformationObjectSerializeShape,
} from "./information-object/sequence"
import { ParseError, SerializeError } from "./utils/errors"
import {
  parseLengthPrefix,
  predictLengthPrefixLength,
  serializeLengthPrefix,
} from "./utils/length-prefix"
import type { ParseContext, SerializeContext } from "./utils/parse"

export type ObjectShape = Record<string, AnyOerType>

export type SequenceShape = Record<string, AnyOerType | AnyObjectSetField>

// Takes the type
export type InferObjectParseShape<TShape extends SequenceShape> = {
  [key in keyof ConditionalPick<
    TShape,
    AnyOerType
  >]: TShape[key] extends AnyOerType ? Infer<TShape[key]> : never
}

export type InferObjectSerializeShape<TShape extends SequenceShape> = {
  [key in keyof ConditionalExcept<
    // We use ConditionalExcept to remove any constants from the serialize shape
    ConditionalPick<TShape, AnyOerType>,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    OerConstant<any, any> | OerOptional<any, any>
  >]: TShape[key] extends AnyOerType ? InferSerialize<TShape[key]> : never
} & {
  [key in keyof ConditionalPick<
    TShape,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    OerOptional<any, any>
  >]?: TShape[key] extends AnyOerType ? InferSerialize<TShape[key]> : never
}

export type InferExtendedSequenceParseShape<
  TConfig extends ExtendedSequenceShape,
> = InferObjectParseShape<TConfig["root"]> &
  InferInformationObjectParseShape<TConfig["root"]> &
  Partial<{
    [key in keyof TConfig["extensions"]]: TConfig["extensions"] extends OerType<
      infer K,
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      any
    >
      ? K
      : InferObjectParseShape<TConfig["extensions"][key]>
  }>

export type InferExtendedSequenceSerializeShape<
  TConfig extends ExtendedSequenceShape,
> = InferObjectSerializeShape<TConfig["root"]> &
  InferInformationObjectSerializeShape<TConfig["root"]> & {
    [key in keyof TConfig["extensions"]]?: TConfig["extensions"][key] extends OerType<
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      any,
      infer K
    >
      ? K
      : InferObjectSerializeShape<TConfig["extensions"][key]>
  }

export interface ExtendedSequenceShape {
  root: SequenceShape
  isExtensible: boolean
  extensions: Record<string, ObjectShape>
}

export class OerSequence<TShape extends ExtendedSequenceShape> extends OerType<
  InferExtendedSequenceParseShape<TShape>,
  InferExtendedSequenceSerializeShape<TShape>
> {
  private rootEntries: Map<string, AnyOerType | AnyObjectSetField>
  private isExtensible: boolean
  private extensions: Map<string, AnyOerType | Map<string, AnyOerType>>

  private rootOptionalFields: string[]

  /**
   * This map tracks which object of an information object set has been selected during parsing.
   *
   * When a discriminant field (something like a type or ID) is parsed, this map is updated to
   * reflect the index of the object in the corresponding object set that was selected.
   *
   * Later, when an open type is parsed, this information is used to select the correct inner type.
   */
  private informationObjectSetState: InformationObjectSetStateMap = new Map()

  constructor(readonly sequenceShape: TShape) {
    super()

    this.rootEntries = new Map(Object.entries(sequenceShape.root))

    this.extensions = new Map(
      Object.entries(sequenceShape.extensions).map(([key, value]) => [
        key,
        value instanceof OerType ? value : new Map(Object.entries(value)),
      ]),
    )

    this.isExtensible = sequenceShape.isExtensible || this.extensions.size > 0

    this.rootOptionalFields = [...this.rootEntries]
      .filter(([, value]) => value instanceof OerOptional)
      .map((v) => v[0])
  }

  clone() {
    return new OerSequence(this.sequenceShape)
  }

  parseWithContext(context: ParseContext, offset: number) {
    const { uint8Array, allowNoncanonical } = context
    let cursor = 0
    this.informationObjectSetState.clear()

    // --- Preamble ---

    const optionalFieldsPresence = new Map<string, boolean>()
    let extensionBit = false

    if (this.isExtensible || this.rootOptionalFields.length > 0) {
      const firstByte = uint8Array[offset]
      if (firstByte == undefined) {
        return new ParseError(
          "unable to read sequence preamble - end of buffer",
          uint8Array,
          offset,
        )
      }
      extensionBit = this.isExtensible && Boolean(firstByte & 0b1000_0000)
      const preambleBitLength =
        (this.isExtensible ? 1 : 0) + this.rootOptionalFields.length
      const preambleByteLength = Math.ceil(preambleBitLength / 8)

      if (uint8Array.length < offset + preambleByteLength) {
        return new ParseError(
          `unable to read sequence preamble - end of buffer`,
          uint8Array,
          uint8Array.length,
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
          offset + preambleByteLength - 1,
        )
      }

      cursor += preambleByteLength
    }

    // --- Root ---

    const resultObject: Record<string, unknown> = {}
    for (const [key, oer] of this.rootEntries) {
      if (oer instanceof OerOptional && !optionalFieldsPresence.get(key)) {
        // If there is no default, oer._default will be undefined, which just so happens to be the value that we want to return in that case
        resultObject[key] = oer.defaultValue
        continue
      }

      const result = oer.parseWithContext(
        context,
        offset + cursor,
        this.informationObjectSetState,
      )
      if (result instanceof ParseError) {
        return result
      }
      resultObject[key] = result[0]
      cursor += result[1]
    }

    // --- Extension Presence Bitmap ---

    const extensionPresence: string[] = []
    if (extensionBit) {
      const extensionPresenceLengthParseResult = parseLengthPrefix(
        context,
        offset + cursor,
      )

      if (extensionPresenceLengthParseResult instanceof ParseError) {
        return extensionPresenceLengthParseResult
      }

      const [extensionPresenceLength, extensionPresenceLengthOfLength] =
        extensionPresenceLengthParseResult

      cursor += extensionPresenceLengthOfLength

      if (extensionPresenceLength < 2) {
        return new ParseError(
          "invalid extension presence bitmap - expected at least two bytes",
          uint8Array,
          offset + cursor,
        )
      }

      const unusedBits = uint8Array[offset + cursor]!

      if (unusedBits > 7) {
        return new ParseError(
          "invalid extension presence bitmap - unused bits value cannot be greater than 7",
          uint8Array,
          offset + cursor,
        )
      }

      cursor += 1

      const extensionPresenceBitLength =
        (extensionPresenceLength - 1) * 8 - unusedBits

      let index = 0
      for (const extensionKey of this.extensions.keys()) {
        if (index >= extensionPresenceBitLength) break
        if (
          (uint8Array[offset + Math.floor(index / 8)]! >> (7 - (index % 8))) &
          1
        ) {
          extensionPresence.push(extensionKey)
        }
        index++
      }

      cursor += extensionPresenceLength - 1
    }

    // --- Extensions ---

    for (const extensionKey of extensionPresence) {
      const extensionLengthParseResult = parseLengthPrefix(
        context,
        offset + cursor,
      )

      if (extensionLengthParseResult instanceof ParseError) {
        return extensionLengthParseResult
      }

      const [extensionLength, extensionLengthOfLength] =
        extensionLengthParseResult

      cursor += extensionLengthOfLength

      const extension = this.extensions.get(extensionKey)

      if (extension instanceof OerType) {
        const extensionParseResult = extension.parseWithContext(
          context,
          offset + cursor,
        )

        if (extensionParseResult instanceof ParseError) {
          return extensionParseResult
        }

        resultObject[extensionKey] = extensionParseResult[0]

        if (extensionParseResult[1] > extensionLength) {
          return new ParseError(
            "extension value extends beyond extension length",
            uint8Array,
            offset + cursor + extensionLength,
          )
        }

        if (!allowNoncanonical && extensionLength > extensionParseResult[1]) {
          return new ParseError(
            "extra bytes inside of extension",
            uint8Array,
            offset + cursor,
          )
        }
      } else {
        throw new TypeError("Unreachable")
      }

      cursor += extensionLength
    }

    return [
      resultObject as InferExtendedSequenceParseShape<TShape>,
      cursor,
    ] as const
  }

  serializeWithContext(input: InferExtendedSequenceSerializeShape<TShape>) {
    let totalLength = 0
    this.informationObjectSetState.clear()

    // --- Preamble ---
    const preambleBitLength =
      (this.isExtensible ? 1 : 0) + this.rootOptionalFields.length
    const preambleLength = Math.ceil(preambleBitLength / 8)
    totalLength += preambleLength

    // --- Root ---

    const serializers: (Serializer | undefined)[] = Array.from({
      length: this.rootEntries.size,
    })
    {
      let index = 0
      for (const [key, oer] of this.rootEntries) {
        if (oer instanceof OerOptional && !(key in input)) {
          continue
        }

        const serializer = oer.serializeWithContext(
          (input as Record<string, unknown>)[key],
          this.informationObjectSetState,
        )

        if (serializer instanceof SerializeError) {
          return serializer
        }

        totalLength += serializer.size
        serializers[index++] = serializer
      }
    }

    // --- Extension Presence Bitmap ---

    let extensionBit = false
    {
      let maxExtensionIndex = 0

      let index = 0
      for (const key of this.extensions.keys()) {
        if (key in input) {
          extensionBit = true
          maxExtensionIndex = index
        }
        index++
      }

      if (extensionBit) {
        const extensionPresenceLength =
          1 + Math.ceil((maxExtensionIndex + 1) / 8)
        const extensionPresenceLengthOfLength = predictLengthPrefixLength(
          extensionPresenceLength,
        )

        if (extensionPresenceLengthOfLength instanceof SerializeError) {
          return extensionPresenceLengthOfLength
        }

        totalLength += extensionPresenceLengthOfLength + extensionPresenceLength

        const serializer = (context: SerializeContext, offset: number) => {
          const { uint8Array } = context
          const lengthSerializeResult = serializeLengthPrefix(
            extensionPresenceLength,
            uint8Array,
            offset,
          )

          if (lengthSerializeResult instanceof SerializeError) {
            return lengthSerializeResult
          }

          // Unused bits
          uint8Array[offset + extensionPresenceLengthOfLength] =
            7 - (maxExtensionIndex % 8)

          let index = 0
          const bitmapOffset = offset + extensionPresenceLengthOfLength + 1
          for (const key of this.extensions.keys()) {
            if (index > maxExtensionIndex) break

            if (key in input) {
              uint8Array[bitmapOffset + Math.floor(index / 8)]! |=
                1 << (7 - (index % 8))
            }
            index++
          }

          return
        }
        serializer.size =
          extensionPresenceLengthOfLength + extensionPresenceLength

        serializers.push(serializer)
      }
    }

    // --- Extensions ---

    for (const [key, extension] of this.extensions) {
      if (!(key in input)) continue

      if (extension instanceof OerType) {
        const innerSerializer = extension.serializeWithContext(input[key])

        if (innerSerializer instanceof SerializeError) {
          return innerSerializer
        }

        const extensionLengthOfLength = predictLengthPrefixLength(
          innerSerializer.size,
        )

        if (extensionLengthOfLength instanceof SerializeError) {
          return extensionLengthOfLength
        }

        const serializer = (context: SerializeContext, offset: number) => {
          const lengthSerializeResult = serializeLengthPrefix(
            innerSerializer.size,
            context.uint8Array,
            offset,
          )

          if (lengthSerializeResult instanceof SerializeError) {
            return lengthSerializeResult
          }

          innerSerializer(context, offset + extensionLengthOfLength)

          return
        }
        serializer.size = extensionLengthOfLength + innerSerializer.size
        serializers.push(serializer)
        totalLength += extensionLengthOfLength + innerSerializer.size
      }
    }

    const serializer = (context: SerializeContext, offset: number) => {
      const { uint8Array } = context

      let cursor = 0

      // --- Preamble ---

      if (preambleBitLength > 0) {
        if (extensionBit) {
          uint8Array[offset] |= 0b1000_0000
        }

        for (const [
          index,
          optionalFieldName,
        ] of this.rootOptionalFields.entries()) {
          if (optionalFieldName in input) {
            const bitIndex = (this.isExtensible ? 1 : 0) + index
            uint8Array[offset + Math.floor(bitIndex / 8)]! |=
              1 << (7 - (bitIndex % 8))
          }
        }
        cursor += preambleLength
      }

      // --- Everything else ---

      for (const serializer of serializers) {
        if (!serializer) continue

        serializer(context, offset + cursor)
        cursor += serializer.size
      }
    }
    serializer.size = totalLength
    return serializer
  }

  extensible() {
    this.isExtensible = true
    return this
  }

  extend<
    TExtensions extends Record<string, AnyOerType | Record<string, AnyOerType>>,
  >(
    extensions: TExtensions,
  ): OerSequence<{
    root: TShape["root"]
    isExtensible: true
    extensions: TShape["extensions"] & TExtensions
  }> {
    const newSequence = new OerSequence({
      root: this.sequenceShape.root,
      isExtensible: true,
      extensions: {
        ...this.sequenceShape.extensions,
        ...extensions,
      } as TShape["extensions"] & TExtensions,
    })
    return newSequence
  }
}

export const sequence = <TRootShape extends SequenceShape>(
  sequenceShape: TRootShape,
) => {
  return new OerSequence({
    root: sequenceShape,
    isExtensible: false,
    extensions: {},
  })
}
