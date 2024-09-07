import type { CamelCase, SnakeCase } from "type-fest"

import type {
  SqliteDataType,
  SqliteToTypescriptTypeMap,
} from "./sqlite-datatypes"

export interface ScalarDescriptionGenerics {
  name: string | undefined
  sqliteType: SqliteDataType
  writeType: NonNullable<unknown>
  readType: NonNullable<unknown>
  defaultValueType: unknown
}

export interface DefaultScalarDescriptionGenerics {
  name: undefined
  sqliteType: "TEXT"
  writeType: string
  readType: string
  defaultValueType: undefined
}

export interface ScalarDescription<
  T extends ScalarDescriptionGenerics = ScalarDescriptionGenerics,
> {
  name: T["name"]

  type: T["sqliteType"]

  defaultValue: T["defaultValueType"]

  /**
   * A transformation function that is applied when writing to the database.
   */
  serialize(
    this: void,
    value: T["writeType"],
  ): SqliteToTypescriptTypeMap[T["sqliteType"]]

  /**
   * A transformation function that is applied when reading from the database.
   */
  deserialize(
    this: void,
    value: SqliteToTypescriptTypeMap[T["sqliteType"]],
  ): T["readType"]
}

export interface ScalarDescriptionBuilder<
  T extends ScalarDescriptionGenerics = ScalarDescriptionGenerics,
> {
  description: ScalarDescription<T>

  name<TName extends string>(
    name: TName,
  ): ScalarDescriptionBuilder<Omit<T, "name"> & { name: TName }>

  type<TType extends SqliteDataType>(
    type: TType,
  ): ScalarDescriptionBuilder<
    Omit<T, "sqliteType" | "writeType" | "readType"> & {
      sqliteType: TType
      writeType: SqliteToTypescriptTypeMap[TType]
      readType: SqliteToTypescriptTypeMap[TType]
    }
  >

  serialize<TWriteType extends NonNullable<unknown>>(
    serializer: (
      value: TWriteType,
    ) => SqliteToTypescriptTypeMap[T["sqliteType"]],
  ): ScalarDescriptionBuilder<
    Omit<T, "writeType"> & {
      writeType: TWriteType
    }
  >

  deserialize<TReadType extends NonNullable<unknown>>(
    deserializer: (
      value: SqliteToTypescriptTypeMap[T["sqliteType"]],
    ) => TReadType,
  ): ScalarDescriptionBuilder<
    Omit<T, "readType"> & {
      readType: TReadType
    }
  >

  defaultValue<TDefaultValueType>(
    defaultValue: TDefaultValueType,
  ): ScalarDescriptionBuilder<
    Omit<T, "defaultValueType"> & { defaultValueType: TDefaultValueType }
  >
}

export type InferScalarReadType<T extends ScalarDescriptionBuilder> =
  T extends ScalarDescriptionBuilder<infer TGenerics> ?
    TGenerics["readType"] | TGenerics["defaultValueType"]
  : never

export type InferScalarWriteType<T extends ScalarDescriptionBuilder> =
  T extends ScalarDescriptionBuilder<infer TGenerics> ? TGenerics["writeType"]
  : never

export type InferScalarSqliteType<T extends ScalarDescriptionBuilder> =
  T extends ScalarDescriptionBuilder<infer TGenerics> ?
    SqliteToTypescriptTypeMap[TGenerics["sqliteType"]] | undefined
  : never

/**
 * Helper type to ensure that a set of scalars are properly namespaced.
 *
 * @remarks
 *
 * This type will match an object of scalars which satisfy the following conditions:
 *
 * - Each key is a string which is the camelCase version of the namespace.
 * - Each value is a scalar description builder.
 * - Each scalar description builder has a name which is the snake_case version of the namespace followed by a dot.
 *
 * @example
 *
 * ```ts
 * const scalars = {
 *  myNamespaceScalar: scalar().type("TEXT").name("my_namespace.scalar"),
 * } as const satisfies NamespacedScalars<"myNamespace">
 * ```
 */
export type NamespacedScalars<TNamespace extends string> = Record<
  `${CamelCase<TNamespace>}${string}`,
  ScalarDescriptionBuilder<
    ScalarDescriptionGenerics & { name: `${SnakeCase<TNamespace>}.${string}` }
  >
>
