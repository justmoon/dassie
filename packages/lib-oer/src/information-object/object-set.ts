import type { AnyOerType, Infer, OerType, Serializer } from "../base-type"
import { octetString } from "../octet-string"
import { ParseError, SerializeError } from "../utils/errors"
import type { ParseContext } from "../utils/parse"
import {
  type ClassDefinitionShape,
  type InformationObjectClass,
  openType,
} from "./class"

export type InformationObjectShape<
  TClassDefinition extends ClassDefinitionShape
> = {
  [TKey in keyof TClassDefinition]: TClassDefinition[TKey] extends AnyOerType
    ? Infer<TClassDefinition[TKey]>
    : AnyOerType
}
export type InformationObjectSetShape<
  TClassDefinition extends ClassDefinitionShape
> = readonly InformationObjectShape<TClassDefinition>[]

export type InformationObjectSet<
  TClassDefinition extends ClassDefinitionShape,
  TObjectSet extends InformationObjectSetShape<TClassDefinition>
> = {
  _class: InformationObjectClass<TClassDefinition>
  _objectSet: TObjectSet
} & {
  [K in keyof TClassDefinition]: ObjectSetField<TClassDefinition, TObjectSet, K>
}

export type AnyObjectSet = InformationObjectSet<
  ClassDefinitionShape,
  InformationObjectSetShape<ClassDefinitionShape>
>

export interface ObjectSetField<
  TClassDefinition extends ClassDefinitionShape,
  TObjectSet extends InformationObjectSetShape<TClassDefinition>,
  TField extends keyof TClassDefinition,
  TFieldType extends TClassDefinition[TField] = TClassDefinition[TField]
> {
  _class: InformationObjectClass<TClassDefinition>
  _objectSet: TObjectSet
  _field: TField
  _type: TFieldType
  parseWithContext: (
    context: ParseContext,
    offset: number,
    informationObjectSetState: InformationObjectSetStateMap
  ) => readonly [value: unknown, length: number] | ParseError
  serializeWithContext: (
    value: unknown,
    informationObjectSetState: InformationObjectSetStateMap
  ) => SerializeError | Serializer
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type AnyObjectSetField = ObjectSetField<any, any, string>

export type InformationObjectSetStateMap = Map<
  InformationObjectSetShape<ClassDefinitionShape>,
  number
>

export const defineObjectSet = <
  TClassDefinition extends ClassDefinitionShape,
  TObjectSet extends InformationObjectSetShape<TClassDefinition>
>(
  informationObjectClass: InformationObjectClass<TClassDefinition>,
  objectSetShape: TObjectSet
): InformationObjectSet<TClassDefinition, TObjectSet> => {
  return {
    _class: informationObjectClass,
    _objectSet: objectSetShape,
    ...Object.fromEntries(
      Object.entries(informationObjectClass._definition).map(([key, value]) => [
        key,
        defineObjectSetField(
          informationObjectClass,
          objectSetShape,
          key,
          value as TClassDefinition[string & keyof TClassDefinition]
        ),
      ])
    ),
  } as InformationObjectSet<TClassDefinition, TObjectSet>
}

const defineObjectSetField = <
  TClassDefinition extends ClassDefinitionShape,
  TObjectSet extends InformationObjectSetShape<TClassDefinition>,
  TField extends string & keyof TClassDefinition,
  TFieldType extends TClassDefinition[TField] = TClassDefinition[TField]
>(
  informationObjectClass: InformationObjectClass<TClassDefinition>,
  objectSetShape: TObjectSet,
  field: TField,
  type: TFieldType
): ObjectSetField<TClassDefinition, TObjectSet, TField, TFieldType> => {
  if (type === openType) {
    const openTypeWrapper = octetString()

    return {
      _class: informationObjectClass,
      _objectSet: objectSetShape,
      _field: field,
      _type: type,
      parseWithContext: (context, offset, informationObjectSetState) => {
        const state = informationObjectSetState.get(objectSetShape)

        if (state === undefined) {
          return new ParseError(
            "Cannot parse open type unless an identifier field appears before it in the same sequence",
            context.uint8Array,
            offset
          )
        }

        const wrapperParseResult = openTypeWrapper.parseWithContext(
          context,
          offset
        )

        if (wrapperParseResult instanceof ParseError) {
          return wrapperParseResult
        }

        const lengthOfLength =
          wrapperParseResult[1] - wrapperParseResult[0].byteLength

        const innerValue = (
          objectSetShape[state]![field] as AnyOerType
        ).parseWithContext(context, offset + lengthOfLength)

        if (innerValue instanceof ParseError) {
          return innerValue
        }

        if (
          !context.allowNoncanonical &&
          innerValue[1] < wrapperParseResult[0].byteLength
        ) {
          return new ParseError(
            "extra bytes inside of open type",
            context.uint8Array,
            offset + lengthOfLength + innerValue[1]
          )
        }

        return [innerValue[0], wrapperParseResult[1]]
      },
      serializeWithContext: (value, informationObjectSetState) => {
        const state = informationObjectSetState.get(objectSetShape)

        if (state === undefined) {
          return new SerializeError(
            "Cannot serialize open type unless an identifier field appears before it in the same sequence"
          )
        }

        const innerValue = (
          objectSetShape[state]![field] as AnyOerType
        ).serializeWithContext(value)

        if (innerValue instanceof SerializeError) {
          return innerValue
        }

        return openTypeWrapper.serializeWithContext(innerValue)
      },
    }
  }

  const serializers = objectSetShape.map((x) => {
    if (x[field] === undefined) {
      throw new TypeError(`Missing required field "${field}"`)
    }
    const serializeResult = type.serializeWithContext(x[field])

    if (serializeResult instanceof SerializeError) {
      // eslint-disable-next-line unicorn/prefer-type-error
      throw new Error(
        `Cannot define object set because the value "${String(
          x[field]
        )}" for field "${field}" failed to serialize`,
        { cause: serializeResult }
      )
    }

    return serializeResult
  })

  return {
    _class: informationObjectClass,
    _objectSet: objectSetShape,
    _field: field,
    _type: type,
    parseWithContext(context, offset, informationObjectSetState) {
      const parseResult = (type as OerType<unknown>).parseWithContext(
        context,
        offset
      )

      if (parseResult instanceof ParseError) {
        return parseResult
      }

      const [value, length] = parseResult

      const index = objectSetShape.findIndex(
        (object) => object[field] === value
      )

      if (index === -1) {
        return new ParseError(
          `Unknown value in identifier field`,
          context.uint8Array,
          offset
        )
      }

      informationObjectSetState.set(objectSetShape, index)

      return [value, length]
    },
    serializeWithContext(value, informationObjectSetState) {
      const index = objectSetShape.findIndex(
        (object) => object[field] === value
      )

      if (index === -1) {
        return new SerializeError(
          `Cannot serialize value "${String(
            value
          )}" because it is not a member of the object set`
        )
      }

      informationObjectSetState.set(objectSetShape, index)

      return serializers[index]!
    },
  }
}
