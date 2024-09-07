import type { Generated } from "kysely"

import type { ColumnDescription, InferColumnTypescriptType } from "./column"
import type { TableDescription } from "./table"

export type InferKyselySchema<
  TTables extends Record<string, TableDescription> | undefined,
> =
  TTables extends Record<string, TableDescription> ?
    {
      [K in keyof TTables as TTables[K]["name"]]: {
        [KColumn in keyof TTables[K]["columns"]]: TTables[K]["columns"][KColumn] extends (
          ColumnDescription
        ) ?
          InferColumnTypescriptType<TTables[K]["columns"][KColumn]>
        : never
      } & {
        rowid: Generated<bigint>
      }
    }
  : Record<string, never>
