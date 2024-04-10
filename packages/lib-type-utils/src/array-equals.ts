export const arrayEquals = (a: readonly unknown[], b: readonly unknown[]) =>
  Array.isArray(a) &&
  Array.isArray(b) &&
  a.length === b.length &&
  a.every((value, index) => b[index] === value)
