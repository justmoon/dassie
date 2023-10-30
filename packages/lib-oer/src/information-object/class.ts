import type { AnyOerType } from "../base-type"

export const openType = Symbol("ASN.1 Open Type")

export type ClassDefinitionShape = Record<string, AnyOerType | typeof openType>

export interface InformationObjectClass<
  TDefinition extends ClassDefinitionShape,
> {
  _definition: TDefinition
}

export const defineClass = <TDefinition extends ClassDefinitionShape>(
  definition: TDefinition,
): InformationObjectClass<TDefinition> => {
  return { _definition: definition }
}
