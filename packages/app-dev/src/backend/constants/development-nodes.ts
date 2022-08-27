import type { ReadonlyDeep } from "type-fest"

/**
 * Defines a network topology. Each array represents a node in the network and specifies the indices of the nodes that it will try to peer with.
 */
export type Template = ReadonlyDeep<number[][]>

export const PEERS: Template = [[], [0], [0, 1], [0, 1, 2], [1], [4]] as const
