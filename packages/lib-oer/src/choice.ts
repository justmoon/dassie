import type { Simplify } from "type-fest"

import { AnyOerType, Infer, InferSerialize, OerType } from "./base-type"
import type { ObjectShape } from "./sequence"
import { ParseError, SerializeError } from "./utils/errors"
import type { ParseContext, SerializeContext } from "./utils/parse"
import {
  TAG_MARKER_CONTEXT,
  TagMarker,
  parseTag,
  predictTagLength,
  serializeTag,
  tagMarkerClassMap,
} from "./utils/tag"

/**
 * Change all properties of an interface to type never
 */
type Never<T> = { [P in keyof T]?: never }

/**
 * Returns a union of interfaces where each member is an object containing a single property which is of such type as inferred from the corresponding member of the input interface.
 */
type InferOptionValues<T extends Record<string, AnyOerType>> = {
  [K in keyof T]-?: { [P in K]: Infer<T[P]> } & Never<
    Pick<T, Exclude<keyof T, K>>
  >
}[keyof T]

type InferOptionSerializeValues<T extends Record<string, AnyOerType>> = {
  [K in keyof T]-?: { [P in K]: InferSerialize<T[P]> } & Never<
    Pick<T, Exclude<keyof T, K>>
  >
}[keyof T]

// Because JavaScript's bitwise operators are 32 bits and we are using two bits for the tag class, we only have 30 bits available to encode the tag value
// This should be plenty though since tag values are usually much smaller.
const MAX_SAFE_TAG_VALUE = Number.MAX_SAFE_INTEGER >>> 2

export class OerChoice<TOptions extends ObjectShape> extends OerType<
  Simplify<InferOptionValues<TOptions>>,
  Simplify<InferOptionSerializeValues<TOptions>>
> {
  // Tags are encoded here as an integer with the class marker in the lowest two bits and the tag value shifted two bits to the right.
  private readonly keyToTagMap = new Map<keyof TOptions, number>()
  private readonly tagToKeyMap = new Map<number, keyof TOptions>()

  /**
   * A string containing the names of the choices and their corresponding tags, separated by commas. Used for debug messages.
   */
  private readonly setHint: string

  constructor(readonly options: TOptions) {
    super()

    // Automatic tagging is enabled if none of the values are explicitly tagged
    const isAutomaticallyTagged = Object.values(options).every(
      (oer) => oer._tag == undefined
    )
    let index = 0

    for (const [key, oer] of Object.entries(options)) {
      const tag = isAutomaticallyTagged
        ? (index++ << 2) | TAG_MARKER_CONTEXT
        : oer._tag != undefined
        ? (oer._tag[0] << 2) | oer._tag[1]
        : undefined

      if (tag == undefined) {
        throw new Error(
          `OER choice: ${key} is not tagged and automatic tagging is disabled`
        )
      }

      if (tag > MAX_SAFE_TAG_VALUE) {
        throw new Error(
          `OER choice: ${key} has a tag value of ${tag} which is too large`
        )
      }

      if (this.tagToKeyMap.has(tag)) {
        throw new Error(
          `OER choice: ${key} has the same tag value as ${String(
            this.tagToKeyMap.get(tag)
          )}`
        )
      }

      this.keyToTagMap.set(key, tag)
      this.tagToKeyMap.set(tag, key)
    }

    Object.fromEntries(
      Object.entries(options).map(([key, oer]) => {
        return [
          key,
          isAutomaticallyTagged
            ? ([index++, TAG_MARKER_CONTEXT] as const)
            : oer._tag,
        ]
      })
    )

    this.setHint = Object.entries(this.keyToTagMap)
      .map(([key, tag]) => `${key}(${String(tag)})`)
      .join(",")
  }

  clone() {
    return new OerChoice(this.options)
  }

  parseWithContext(context: ParseContext, offset: number) {
    const tagResult = parseTag(context, offset)

    if (tagResult instanceof ParseError) {
      return tagResult
    }

    const [tag, tagClass, tagLength] = tagResult

    if (tag > MAX_SAFE_TAG_VALUE) {
      return new ParseError(
        `unable to parse choice value - tag value ${tag} is too large`,
        context.uint8Array,
        offset
      )
    }

    const key = this.tagToKeyMap.get((tag << 2) | tagClass)

    if (key == undefined) {
      return new ParseError(
        `unable to read choice value - tag ${tagMarkerClassMap[
          tagClass
        ].toUpperCase()} ${tag} not in set ${this.setHint}`,
        context.uint8Array,
        offset
      )
    }

    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    const oer = this.options[key]!

    const result = oer.parseWithContext(context, offset + tagLength)

    if (result instanceof ParseError) {
      return result
    }

    return [
      { [key]: result[0] } as Simplify<InferOptionValues<TOptions>>,
      tagLength + result[1],
    ] as const
  }

  serializeWithContext(input: Simplify<InferOptionSerializeValues<TOptions>>) {
    const key = Object.keys(input)[0]

    if (key == undefined) {
      return new SerializeError(
        `unable to serialize choice value - no option selected`
      )
    }

    const tag = this.keyToTagMap.get(key)

    if (tag == undefined) {
      return new SerializeError(
        `unable to serialize choice value - option ${key} not in set ${this.setHint}`
      )
    }

    const tagValue = tag >>> 2
    const tagClass = (tag & 3) as TagMarker

    const oer = this.options[key]

    if (oer == undefined) {
      return new SerializeError(
        `unable to serialize choice value - option ${key} not in set ${this.setHint}`
      )
    }

    const tagLength = predictTagLength(tagValue)

    if (tagLength instanceof SerializeError) {
      return tagLength
    }

    const valueSerializer = oer.serializeWithContext(input[key])

    if (valueSerializer instanceof SerializeError) {
      return valueSerializer
    }

    const serializer = (context: SerializeContext, offset: number) => {
      const { uint8Array } = context

      const tagSerializeResult = serializeTag(
        tagValue,
        tagClass,
        uint8Array,
        offset
      )

      if (tagSerializeResult instanceof SerializeError) {
        return tagSerializeResult
      }

      const serializeResult = valueSerializer(context, offset + tagLength)

      if (serializeResult instanceof SerializeError) {
        return serializeResult
      }

      return
    }
    serializer.size = tagLength + valueSerializer.size
    return serializer
  }
}

export const choice = <TOptions extends ObjectShape>(options: TOptions) => {
  return new OerChoice(options)
}
