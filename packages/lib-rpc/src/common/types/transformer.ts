export interface Transformer {
  stringify(value: unknown): string
  parse(value: string): unknown
}
