import { ensureUint8Array } from "./utils/ensure-uint8array"
import { ParseError, SerializeError } from "./utils/errors"
import type { ParseContext, SerializeContext } from "./utils/parse"
import { type TagClass, type TagMarker, tagClassMarkerMap } from "./utils/tag"

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type AnyOerType = OerType<any>

export type Infer<TOerType extends AnyOerType> = TOerType extends OerType<
  infer T,
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  any
>
  ? T
  : never

export type InferSerialize<TOerType extends AnyOerType> =
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  TOerType extends OerType<any, infer T> ? T : never

export interface ParseOptions {
  allowNoncanonical?: boolean
}

export interface Serializer {
  (context: SerializeContext, offset: number): SerializeError | void
  size: number
}

export abstract class OerType<TParseValue, TSerializeValue = TParseValue> {
  _tag: readonly [tagValue: number, tagClass: TagMarker] | undefined
  _unique = false

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
    | { success: false; error: ParseError } {
    const context: ParseContext = {
      dataView: new DataView(input.buffer, input.byteOffset, input.byteLength),
      uint8Array: ensureUint8Array(input),
      allowNoncanonical: options.allowNoncanonical ?? false,
    }
    const result = this.parseWithContext(context, offset)

    if (result instanceof ParseError) {
      return { success: false, error: result }
    }

    if (!context.allowNoncanonical && result[1] !== input.byteLength - offset) {
      return {
        success: false,
        error: new ParseError(
          "non-canonical encoding - additional bytes present after the expected end of data",
          input,
          result[1]
        ),
      }
    }

    return { success: true, value: result[0], length: result[1] }
  }

  parseOrThrow(
    input: Uint8Array,
    offset = 0,
    options: ParseOptions = {}
  ): TParseValue {
    const result = this.parse(input, offset, options)

    if (result.success) {
      return result.value
    }

    throw result.error
  }

  serialize(
    value: TSerializeValue
  ):
    | { success: true; value: Uint8Array }
    | { success: false; error: SerializeError } {
    const serializer = this.serializeWithContext(value)

    if (serializer instanceof SerializeError) {
      return { success: false, error: serializer }
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
      ? { success: false, error: result }
      : { success: true, value: uint8Array }
  }

  serializeOrThrow(value: TSerializeValue): Uint8Array {
    const result = this.serialize(value)

    if (result.success) {
      return result.value
    }

    throw result.error
  }

  tag(tagValue: number, tagClass: Exclude<TagClass, "universal"> = "context") {
    const copy = this.clone()
    copy._tag = [tagValue, tagClassMarkerMap[tagClass]] as const
    return copy
  }

  /**
   * Mark this field as optional.
   *
   * @remarks
   *
   * If the field is not present in the encoding, the value will be `undefined`.
   */
  optional(): OerOptional<TParseValue, TSerializeValue> {
    return new OerOptional(this, undefined)
  }

  /**
   * Provide a default value for this field.
   *
   * @remarks
   *
   * If the field is not present in the encoding, the default value will be used.
   */
  default(value: TParseValue): OerOptional<TParseValue, TSerializeValue> {
    return new OerOptional(this, value)
  }

  /**
   * Mark this field as constant meaning the value must always be the same.
   *
   * @remarks
   *
   * If an encoding contains a different value, parsing will fail.
   */
  constant<TParseValue, TSerializeValue>(
    this: OerType<TParseValue, TSerializeValue>,
    value: TSerializeValue
  ): OerConstant<TParseValue, TSerializeValue> {
    return new OerConstant(this, value)
  }

  /**
   * Marks this type as UNIQUE, which means it will be treated as an identifier field in the context of information objects.
   *
   * @returns A copy of this type with the unique flag set
   */
  unique() {
    const copy = this.clone()
    copy._unique = true
    return copy
  }

  /**
   * Provide a validation function which narrows the type of the parsed value.
   *
   * @remarks
   *
   * This is useful to create custom types which are a subset of the original type.
   *
   * @example
   *
   * ```typescript
   * type PositiveInteger = number & { __positiveInteger: true }
   * const uint8Type = oer.uint8().refind((value): value is PositiveInteger => value > 0))
   * ```
   *
   * @param predicate - A function which returns true if the value is valid
   * @returns A copy of this type with the validation function applied
   */
  refine<TNewType extends TParseValue & TSerializeValue>(
    predicate: (value: TParseValue) => value is TNewType
  ): OerRefined<TNewType, TParseValue, TSerializeValue> {
    return new OerRefined(this, predicate)
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

  clone(): OerOptional<TParseValue, TSerializeValue> {
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
        { cause: subtypeSerializeResult.error }
      )
    }

    this.serializedValue = subtypeSerializeResult.value

    const subtypeParseResult = subType.parse(this.serializedValue)

    if (!subtypeParseResult.success) {
      throw new TypeError(
        `Failed to parse constant OER type with value '${String(value)}'`,
        { cause: subtypeParseResult.error }
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

export class OerRefined<
  TNarrowedType extends TParseType & TSerializeValue,
  TParseType,
  TSerializeValue
> extends OerType<TNarrowedType, TNarrowedType> {
  constructor(
    readonly subType: OerType<TParseType, TSerializeValue>,
    readonly predicate: (value: TParseType) => value is TNarrowedType
  ) {
    super()
  }

  clone(): OerRefined<TNarrowedType, TParseType, TSerializeValue> {
    return new OerRefined(this.subType, this.predicate)
  }

  parseWithContext(
    context: ParseContext,
    offset: number
  ): ParseError | readonly [TNarrowedType, number] {
    const result = this.subType.parseWithContext(context, offset)

    if (result instanceof ParseError) {
      return result
    }

    const [value, size] = result

    if (!this.predicate(value)) {
      return new ParseError(
        `refinement predicate failed`,
        context.uint8Array,
        offset
      )
    }

    return [value, size]
  }

  serializeWithContext(value: TNarrowedType): SerializeError | Serializer {
    return this.subType.serializeWithContext(value)
  }
}
