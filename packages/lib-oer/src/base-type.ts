import { ParseError, SerializeError } from "./utils/errors"
import type { ParseContext, SerializeContext } from "./utils/parse"
import { TagClass, TagMarker, tagClassMarkerMap } from "./utils/tag"

export type AnyOerType = OerType<unknown>

export type Infer<TOerType extends AnyOerType> = TOerType extends OerType<
  infer T,
  unknown
>
  ? T
  : never

export type InferSerialize<TOerType extends AnyOerType> =
  TOerType extends OerType<unknown, infer T> ? T : never

export interface ParseOptions {
  allowNoncanonical?: boolean
}

export type IntermediateSerializationResult = readonly [
  serialize: (
    context: SerializeContext,
    offset: number
    // eslint-disable-next-line @typescript-eslint/no-invalid-void-type
  ) => SerializeError | void,
  length: number
]

export abstract class OerType<TParseValue, TSerializeValue = TParseValue> {
  _tag: readonly [tagValue: number, tagClass: TagMarker] | undefined
  _optional = false

  abstract parseWithContext(
    context: ParseContext,
    offset: number
  ): readonly [value: TParseValue, length: number] | ParseError
  abstract serializeWithContext(
    value: TSerializeValue
  ): IntermediateSerializationResult | SerializeError

  parse(
    input: Uint8Array,
    offset = 0,
    options: ParseOptions = {}
  ):
    | { success: true; value: TParseValue; length: number }
    | { success: false; failure: ParseError } {
    const context: ParseContext = {
      uint8Array: input,
      dataView: new DataView(input.buffer, input.byteOffset, input.byteLength),
      allowNoncanonical: options.allowNoncanonical ?? false,
    }
    const result = this.parseWithContext(context, offset)

    if (result instanceof ParseError) {
      return { success: false, failure: result }
    }

    if (!context.allowNoncanonical && result[1] !== input.byteLength - offset) {
      return {
        success: false,
        failure: new ParseError(
          "non-canonical encoding - additional bytes present after the expected end of data",
          input,
          result[1]
        ),
      }
    }

    return { success: true, value: result[0], length: result[1] }
  }

  serialize(
    value: TSerializeValue
  ):
    | { success: true; value: Uint8Array }
    | { success: false; failure: SerializeError } {
    const prediction = this.serializeWithContext(value)

    if (prediction instanceof SerializeError) {
      return { success: false, failure: prediction }
    }

    const [serialize, length] = prediction

    const uint8Array = new Uint8Array(length)

    const result = serialize(
      {
        uint8Array,
        dataView: new DataView(
          uint8Array.buffer,
          uint8Array.byteOffset,
          uint8Array.byteLength
        ),
      },
      0
    )

    return result instanceof SerializeError
      ? { success: false, failure: result }
      : { success: true, value: uint8Array }
  }

  tag(
    tagValue: number,
    tagClass: Exclude<TagClass, "universal"> = "context"
  ): this {
    this._tag = [tagValue, tagClassMarkerMap[tagClass]] as const
    return this
  }

  optional(): this {
    this._optional = true
    return this
  }
}
