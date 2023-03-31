export default function* iterateMultiple<T>(
  ...iterables: Iterable<T>[]
): Iterable<T> {
  for (const iterable of iterables) {
    yield* iterable
  }
}
