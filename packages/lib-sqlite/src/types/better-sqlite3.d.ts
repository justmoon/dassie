/* eslint-disable @typescript-eslint/no-explicit-any */
module "better-sqlite3" {
  export type VariableArgumentFunction = (...parameters: never[]) => unknown
  export type ArgumentTypes<F extends VariableArgumentFunction> = F extends (
    ...parameters: infer A
  ) => unknown
    ? A
    : never

  export interface Statement<BindParameters extends unknown[]> {
    database: Database
    source: string
    reader: boolean
    busy: boolean

    run(...parameters: BindParameters): RunResult
    get(...parameters: BindParameters): unknown
    all(...parameters: BindParameters): unknown[]
    iterate(...parameters: BindParameters): IterableIterator<unknown>
    pluck(toggleState?: boolean): this
    expand(toggleState?: boolean): this
    raw(toggleState?: boolean): this
    bind(...parameters: BindParameters): this
    columns(): ColumnDefinition[]
    safeIntegers(toggleState?: boolean): this
  }

  export interface ColumnDefinition {
    name: string
    column: string | null
    table: string | null
    database: string | null
    type: string | null
  }

  export interface Transaction<F extends VariableArgumentFunction> {
    (...parameters: ArgumentTypes<F>): ReturnType<F>
    default(...parameters: ArgumentTypes<F>): ReturnType<F>
    deferred(...parameters: ArgumentTypes<F>): ReturnType<F>
    immediate(...parameters: ArgumentTypes<F>): ReturnType<F>
    exclusive(...parameters: ArgumentTypes<F>): ReturnType<F>
  }

  export interface VirtualTableOptions {
    rows: () => Generator
    columns: string[]
    parameters?: string[] | undefined
    safeIntegers?: boolean | undefined
    directOnly?: boolean | undefined
  }

  export interface Database {
    memory: boolean
    readonly: boolean
    name: string
    open: boolean
    inTransaction: boolean

    prepare<
      TBindParameters extends unknown[] | Record<string, unknown> = unknown[]
    >(
      source: string
    ): TBindParameters extends unknown[]
      ? Statement<TBindParameters>
      : Statement<[TBindParameters]>
    transaction<F extends VariableArgumentFunction>(executor: F): Transaction<F>
    exec(source: string): this
    pragma(source: string, options?: PragmaOptions): unknown
    function(name: string, callback: VariableArgumentFunction): this
    function(
      name: string,
      options: RegistrationOptions,
      callback: VariableArgumentFunction
    ): this
    aggregate(name: string, options: AggregateOptions): this
    loadExtension(path: string): this
    close(): this
    defaultSafeIntegers(toggleState?: boolean): this
    backup(
      destinationFile: string,
      options?: BackupOptions
    ): Promise<BackupMetadata>
    table(name: string, options: VirtualTableOptions): this
    unsafeMode(unsafe?: boolean): this
    serialize(options?: SerializeOptions): Buffer
  }

  export interface DatabaseConstructor {
    new (filename: string | Buffer, options?: Options): Database
    (filename: string, options?: Options): Database
    prototype: Database

    SqliteError: typeof SqliteError
  }

  // declare class SqliteError implements Error {
  //   name: string
  //   message: string
  //   code: string
  //   constructor(message: string, code: string)
  // }

  export interface RunResult {
    changes: number
    lastInsertRowid: bigint
  }

  export interface Options {
    readonly?: boolean | undefined
    fileMustExist?: boolean | undefined
    timeout?: number | undefined
    verbose?:
      | ((message?: string, ...additionalArguments: unknown[]) => void)
      | undefined
    nativeBinding?: string | undefined
  }

  export interface SerializeOptions {
    attached?: string
  }

  export interface PragmaOptions {
    simple?: boolean | undefined
  }

  export interface AggregateOptions extends RegistrationOptions {
    start?: any
    step: (total: any, next: any) => any
    inverse?: ((total: any, dropped: any) => any) | undefined
    result?: ((total: any) => any) | undefined
  }

  export interface BackupMetadata {
    totalPages: number
    remainingPages: number
  }
  export interface BackupOptions {
    progress: (info: BackupMetadata) => number
  }

  declare const Database: DatabaseConstructor
  export = Database
}
