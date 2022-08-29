import { ParseError, SerializeError } from "./utils/errors"
import type { ParseContext, SerializeContext } from "./utils/parse"
import { TagClass, TagMarker, tagClassMarkerMap } from "./utils/tag"

export type AnyOerType = OerType<unknown>

export type Infer<TOerType extends AnyOerType> = TOerType extends OerType<
  infer T,
  never
>
  ? T
  : never

export type InferSerialize<TOerType extends AnyOerType> =
  TOerType extends OerType<unknown, infer T> ? T : never

export interface ParseOptions {
  allowNoncanonical?: boolean
}

export interface Serializer {
  (context: SerializeContext, offset: number): SerializeError | void
  size: number
}

export abstract class OerType<TParseValue, TSerializeValue = TParseValue> {
  _tag: readonly [tagValue: number, tagClass: TagMarker] | undefined

  abstract clone(): OerType<TParseValue, TSerializeValue>

  abstract parseWithContext(
    context: ParseContext,
    offset: number
  ): readonly [value: TParseValue, length: number] | ParseError
  abstract serializeWithContext(
    value: TSerializeValue
  ): Serializer | SerializeError

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
    const serializer = this.serializeWithContext(value)

    if (serializer instanceof SerializeError) {
      return { success: false, failure: serializer }
    }

    const uint8Array = new Uint8Array(serializer.size)

    const result = serializer(
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

  tag(tagValue: number, tagClass: Exclude<TagClass, "universal"> = "context") {
    const copy = this.clone()
    copy._tag = [tagValue, tagClassMarkerMap[tagClass]] as const
    return copy
  }

  optional(): OerOptional<TParseValue, TSerializeValue> {
    return new OerOptional(this, undefined)
  }

  default(value: TParseValue): OerOptional<TParseValue, TSerializeValue> {
    return new OerOptional(this, value)
  }

  constant<TParseValue, TSerializeValue>(
    this: OerType<TParseValue, TSerializeValue>,
    value: TSerializeValue
  ): OerConstant<TParseValue, TSerializeValue> {
    return new OerConstant(this, value)
  }
}

export class OerOptional<TParseValue, TSerializeValue> extends OerType<
  TParseValue,
  TSerializeValue
> {
  constructor(
    readonly subType: OerType<TParseValue, TSerializeValue>,
    readonly defaultValue: TParseValue | undefined
  ) {
    super()
  }

  clone() {
    return new OerOptional(this.subType, this.defaultValue)
  }

  parseWithContext(
    context: ParseContext,
    offset: number
  ): ParseError | readonly [TParseValue, number] {
    return this.subType.parseWithContext(context, offset)
  }

  serializeWithContext(value: TSerializeValue): SerializeError | Serializer {
    return this.subType.serializeWithContext(value)
  }
}

export class OerConstant<TParseValue, TSerializeValue> extends OerType<
  TParseValue,
  undefined
> {
  private readonly serializedValue: Uint8Array
  private readonly parsedValue: TParseValue

  constructor(
    readonly subType: OerType<TParseValue, TSerializeValue>,
    readonly value: TSerializeValue
  ) {
    super()

    const subtypeSerializeResult = subType.serialize(value)

    if (!subtypeSerializeResult.success) {
      throw new TypeError(
        `Failed to serialize constant OER type with value '${String(value)}'`,
        { cause: subtypeSerializeResult.failure }
      )
    }

    this.serializedValue = subtypeSerializeResult.value

    const subtypeParseResult = subType.parse(this.serializedValue)

    if (!subtypeParseResult.success) {
      throw new TypeError(
        `Failed to parse constant OER type with value '${String(value)}'`,
        { cause: subtypeParseResult.failure }
      )
    }

    this.parsedValue = subtypeParseResult.value
  }

  clone() {
    return new OerConstant(this.subType, this.value)
  }

  parseWithContext(
    context: ParseContext,
    offset: number
  ): ParseError | readonly [TParseValue, number] {
    if (context.uint8Array.length < offset + this.serializedValue.length) {
      return new ParseError(
        `unable to read constant value - end of buffer`,
        context.uint8Array,
        context.uint8Array.length
      )
    }

    for (let index = 0; index < this.serializedValue.length; index++) {
      if (context.uint8Array[offset + index] !== this.serializedValue[index]) {
        return new ParseError(
          `expected constant value did not match data while parsing`,
          context.uint8Array,
          offset + index
        )
      }
    }

    return [this.parsedValue, this.serializedValue.length]
  }

  serializeWithContext(): Serializer {
    // eslint-disable-next-line unicorn/consistent-function-scoping
    const serializer = (context: SerializeContext, offset: number) => {
      context.uint8Array.set(this.serializedValue, offset)
    }
    serializer.size = this.serializedValue.length
    return serializer
  }

  override serialize() {
    return super.serialize(undefined)
  }
}
