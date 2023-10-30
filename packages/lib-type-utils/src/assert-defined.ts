export function assertDefined<T>(
  value: T | null | undefined,
): asserts value is T {
  if (value == undefined) {
    throw new Error(`value ${String(value)} must not be null/undefined.`)
  }
}
