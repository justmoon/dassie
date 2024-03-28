import type { ConditionalExcept, ConditionalPick } from "type-fest"

import { isFailure } from "@dassie/lib-type-utils"

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
import { ParseFailure } from "./utils/failures"
import {
  parseLengthPrefix,
  predictLengthPrefixLength,
  serializeLengthPrefix,
} from "./utils/length-prefix"
import type { ParseContext, SerializeContext } from "./utils/parse"

export type ObjectShape = Record<string, AnyOerType>

export type SequenceShape = Record<string, AnyOerType | AnyObjectSetField>

export const extensions = Symbol("extensions")
export type extensions = typeof extensions

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
> = InferObjectParseShape<TConfig> &
  InferInformationObjectParseShape<TConfig> &
  Partial<{
    [key in keyof TConfig[extensions]]: TConfig[extensions] extends OerType<
      infer K,
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      any
    >
      ? K
      : never
  }>

export type InferExtendedSequenceSerializeShape<
  TConfig extends ExtendedSequenceShape,
> = InferObjectSerializeShape<TConfig> &
  InferInformationObjectSerializeShape<TConfig> & {
    [key in keyof TConfig[extensions]]?: TConfig[extensions][key] extends OerType<
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      any,
      infer K
    >
      ? K
      : never
  }

export type AddExtensionsToShape<
  TShape extends ExtendedSequenceShape,
  TExtensions extends ObjectShape,
> = OerSequence<
  {
    [key in Exclude<keyof TShape, extensions>]: TShape[key]
  } & {
    [extensions]: TShape[extensions] extends ObjectShape
      ? TShape[extensions] & TExtensions
      : TExtensions
  }
>

export type ExtendedSequenceShape = SequenceShape & {
  [extensions]?: ObjectShape
}

export class OerSequence<TShape extends ExtendedSequenceShape> extends OerType<
  InferExtendedSequenceParseShape<TShape>,
  InferExtendedSequenceSerializeShape<TShape>
> {
  private rootEntries: Map<string, AnyOerType | AnyObjectSetField>
  private extensions: Map<string, AnyOerType> | undefined

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

    this.rootEntries = new Map(Object.entries(sequenceShape))

    this.extensions = sequenceShape[extensions]
      ? new Map(
          Object.entries(sequenceShape[extensions]).map(([key, value]) => [
            key,
            value,
          ]),
        )
      : undefined

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

    if (this.extensions || this.rootOptionalFields.length > 0) {
      const firstByte = uint8Array[offset]
      if (firstByte == undefined) {
        return new ParseFailure(
          "unable to read sequence preamble - end of buffer",
          uint8Array,
          offset,
        )
      }
      extensionBit = !!this.extensions && Boolean(firstByte & 0b1000_0000)
      const preambleBitLength =
        (this.extensions ? 1 : 0) + this.rootOptionalFields.length
      const preambleByteLength = Math.ceil(preambleBitLength / 8)

      if (uint8Array.length < offset + preambleByteLength) {
        return new ParseFailure(
          `unable to read sequence preamble - end of buffer`,
          uint8Array,
          uint8Array.length,
        )
      }

      for (const [
        index,
        optionalFieldName,
      ] of this.rootOptionalFields.entries()) {
        const bitIndex = index + (this.extensions ? 1 : 0)
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
        return new ParseFailure(
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
      if (isFailure(result)) return result

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

      if (isFailure(extensionPresenceLengthParseResult)) {
        return extensionPresenceLengthParseResult
      }

      const [extensionPresenceLength, extensionPresenceLengthOfLength] =
        extensionPresenceLengthParseResult

      cursor += extensionPresenceLengthOfLength

      if (extensionPresenceLength < 2) {
        return new ParseFailure(
          "invalid extension presence bitmap - expected at least two bytes",
          uint8Array,
          offset + cursor,
        )
      }

      const unusedBits = uint8Array[offset + cursor]!

      if (unusedBits > 7) {
        return new ParseFailure(
          "invalid extension presence bitmap - unused bits value cannot be greater than 7",
          uint8Array,
          offset + cursor,
        )
      }

      cursor += 1

      const extensionPresenceBitLength =
        (extensionPresenceLength - 1) * 8 - unusedBits

      if (this.extensions) {
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
      }

      cursor += extensionPresenceLength - 1
    }

    // --- Extensions ---

    for (const extensionKey of extensionPresence) {
      const extensionLengthParseResult = parseLengthPrefix(
        context,
        offset + cursor,
      )

      if (isFailure(extensionLengthParseResult)) {
        return extensionLengthParseResult
      }

      const [extensionLength, extensionLengthOfLength] =
        extensionLengthParseResult

      cursor += extensionLengthOfLength

      const extension = this.extensions?.get(extensionKey)

      if (extension instanceof OerType) {
        const extensionParseResult = extension.parseWithContext(
          context,
          offset + cursor,
        )
        if (isFailure(extensionParseResult)) return extensionParseResult

        resultObject[extensionKey] = extensionParseResult[0]

        if (extensionParseResult[1] > extensionLength) {
          return new ParseFailure(
            "extension value extends beyond extension length",
            uint8Array,
            offset + cursor + extensionLength,
          )
        }

        if (!allowNoncanonical && extensionLength > extensionParseResult[1]) {
          return new ParseFailure(
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
      (this.extensions ? 1 : 0) + this.rootOptionalFields.length
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
        if (isFailure(serializer)) return serializer

        totalLength += serializer.size
        serializers[index++] = serializer
      }
    }

    // --- Extension Presence Bitmap ---

    let extensionBit = false
    const extensionsMap = this.extensions
    if (extensionsMap) {
      {
        let maxExtensionIndex = 0

        let index = 0
        for (const key of extensionsMap.keys()) {
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

          if (isFailure(extensionPresenceLengthOfLength)) {
            return extensionPresenceLengthOfLength
          }

          totalLength +=
            extensionPresenceLengthOfLength + extensionPresenceLength

          const serializer: Serializer = (
            context: SerializeContext,
            offset: number,
          ) => {
            const { uint8Array } = context
            const lengthSerializeResult = serializeLengthPrefix(
              extensionPresenceLength,
              uint8Array,
              offset,
            )
            if (isFailure(lengthSerializeResult)) return lengthSerializeResult

            // Unused bits
            uint8Array[offset + extensionPresenceLengthOfLength] =
              7 - (maxExtensionIndex % 8)

            let index = 0
            const bitmapOffset = offset + extensionPresenceLengthOfLength + 1
            for (const key of extensionsMap.keys()) {
              if (index > maxExtensionIndex) break

              if (key in input) {
                uint8Array[bitmapOffset + Math.floor(index / 8)] |=
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

      for (const [key, extension] of extensionsMap) {
        if (!(key in input)) continue

        if (extension instanceof OerType) {
          const innerSerializer = extension.serializeWithContext(
            input[key as keyof typeof input],
          )
          if (isFailure(innerSerializer)) return innerSerializer

          const extensionLengthOfLength = predictLengthPrefixLength(
            innerSerializer.size,
          )
          if (isFailure(extensionLengthOfLength)) return extensionLengthOfLength

          const serializer: Serializer = (
            context: SerializeContext,
            offset: number,
          ) => {
            const lengthSerializeResult = serializeLengthPrefix(
              innerSerializer.size,
              context.uint8Array,
              offset,
            )
            if (isFailure(lengthSerializeResult)) return lengthSerializeResult

            return innerSerializer(context, offset + extensionLengthOfLength)
          }
          serializer.size = extensionLengthOfLength + innerSerializer.size
          serializers.push(serializer)
          totalLength += extensionLengthOfLength + innerSerializer.size
        }
      }
    }

    const serializer: Serializer = (
      context: SerializeContext,
      offset: number,
    ) => {
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
            const bitIndex = (this.extensions ? 1 : 0) + index
            uint8Array[offset + Math.floor(bitIndex / 8)] |=
              1 << (7 - (bitIndex % 8))
          }
        }
        cursor += preambleLength
      }

      // --- Everything else ---

      for (const serializer of serializers) {
        if (!serializer) continue

        const result = serializer(context, offset + cursor)
        if (isFailure(result)) return result

        cursor += serializer.size
      }

      return
    }
    serializer.size = totalLength
    return serializer
  }

  extensible() {
    this.extensions ??= new Map()
    return this
  }

  extend<TExtensions extends Record<string, AnyOerType>>(
    newExtensions: TExtensions,
  ): AddExtensionsToShape<TShape, TExtensions> {
    const newSequence = new OerSequence({
      ...this.sequenceShape,
      [extensions]: {
        ...this.sequenceShape[extensions],
        ...newExtensions,
      },
    })
    return newSequence as unknown as AddExtensionsToShape<TShape, TExtensions>
  }
}

export const sequence = <TRootShape extends SequenceShape>(
  sequenceShape: TRootShape,
) => {
  return new OerSequence(sequenceShape)
}
