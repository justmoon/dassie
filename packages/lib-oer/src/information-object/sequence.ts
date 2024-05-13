import type { ConditionalPick } from "type-fest"

import type { AnyOerType, Infer, InferSerialize } from "../base-type"
import type { SequenceShape } from "../sequence"
import type { AnyObjectSetField, ObjectSetField } from "./object-set"

export type InferInformationObjectParseShape<TShape extends SequenceShape> =
  TShape[keyof ConditionalPick<
    TShape,
    AnyObjectSetField
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
  >] extends ObjectSetField<any, infer TObjectSet, string> ?
    {
      [TElement in keyof TObjectSet]: {
        [TKey in keyof ConditionalPick<
          TShape,
          AnyObjectSetField
        >]: TShape[TKey] extends AnyObjectSetField ?
          TObjectSet[TElement][TShape[TKey]["_field"]] extends AnyOerType ?
            Infer<TObjectSet[TElement][TShape[TKey]["_field"]]>
          : TObjectSet[TElement][TShape[TKey]["_field"]]
        : never
      }
    }[number]
  : never

export type InferInformationObjectSerializeShape<TShape extends SequenceShape> =
  TShape[keyof ConditionalPick<
    TShape,
    AnyObjectSetField
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
  >] extends ObjectSetField<any, infer TObjectSet, string> ?
    {
      [TElement in keyof TObjectSet]: {
        [TKey in keyof ConditionalPick<
          TShape,
          AnyObjectSetField
        >]: TShape[TKey] extends AnyObjectSetField ?
          TObjectSet[TElement][TShape[TKey]["_field"]] extends AnyOerType ?
            InferSerialize<TObjectSet[TElement][TShape[TKey]["_field"]]>
          : TObjectSet[TElement][TShape[TKey]["_field"]]
        : never
      }
    }[number]
  : never
