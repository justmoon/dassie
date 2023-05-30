export const isAsyncFunction = (value: unknown) =>
  Object.prototype.toString.call(value) === "[object AsyncFunction]"
