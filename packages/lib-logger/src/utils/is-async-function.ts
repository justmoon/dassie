export const isAsyncFunction = (value: unknown) =>
  typeof value === "function" &&
  Symbol.toStringTag in value &&
  value[Symbol.toStringTag] === "AsyncFunction"
