import { Opaque } from "type-fest"

export type SeedPath = Opaque<string, "SeedPath">

export const SEED_PATH_NODE = "node" as SeedPath
export const SEED_PATH_NODE_LOGIN = "node/login" as SeedPath
