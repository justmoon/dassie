export default function assertDefined<T>(
  value: T | null | undefined
): asserts value is T {
  if (value == undefined) {
    throw new Error(`value ${value} must not be null/undefined.`)
  }
}
