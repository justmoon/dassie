import { isObject } from "./is-object"

export const isThenable = (value: unknown): value is PromiseLike<unknown> =>
  isObject(value) && "then" in value && typeof value["then"] === "function"
