// `keyof never` is the upper bound of all keys that can be used in indexable types, currently `string | number | symbol`
// See: https://github.com/microsoft/TypeScript/issues/33025
export const isObject = (o: unknown): o is Record<keyof never, unknown> =>
  typeof o === "object" && o !== null
