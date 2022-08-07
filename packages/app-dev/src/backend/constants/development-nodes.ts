export type Template = readonly (readonly [source: number, target: number])[]

export const PEERS: Template = [
  [1, 0],
  [2, 0],
  [2, 1],
  [3, 2],
  [4, 1],
  [5, 4],
] as const
