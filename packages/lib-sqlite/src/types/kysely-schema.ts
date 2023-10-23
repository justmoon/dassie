import {
  ColumnDescription,
  ColumnDescriptionGenerics,
  InferColumnTypescriptType,
} from "./column"
import { TableDescription } from "./table"

export type InferKyselySchema<
  TTables extends Record<string, TableDescription> | undefined,
> = TTables extends Record<string, TableDescription>
  ? {
      [K in keyof TTables as TTables[K]["name"]]: {
        [KColumn in keyof TTables[K]["columns"]]: TTables[K]["columns"][KColumn] extends ColumnDescription<ColumnDescriptionGenerics>
          ? InferColumnTypescriptType<TTables[K]["columns"][KColumn]>
          : never
      }
    }
  : Record<string, never>
