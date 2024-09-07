import type { Tagged } from "type-fest"

export type SeedPath = Tagged<string, "SeedPath">

/**
 * Node private Ed25519 key.
 */
export const SEED_PATH_NODE = "node" as SeedPath

/**
 * Secret value which is used by client to prove knowledge of node private key for login purposes.
 */
export const SEED_PATH_NODE_LOGIN = "node/login" as SeedPath

/**
 * Deterministic session token used for auto-login during development.
 */
export const SEED_PATH_DEV_SESSION = "dev/session" as SeedPath

/**
 * Entropy for settlement methods.
 */
export const SEED_PATH_SETTLEMENT_MODULE = "node/settlement-module" as SeedPath
