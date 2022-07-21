import type { Reactor } from "./reactor"

export type Factory<TProduct> = (reactor: Reactor) => TProduct
