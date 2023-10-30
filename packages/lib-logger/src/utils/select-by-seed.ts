export const selectBySeed = <T>(
  colors: readonly [T, ...T[]],
  seed: string,
): T => {
  const hash =
    [...seed].reduce((hash, char) => hash + (char.codePointAt(0) ?? 0), 0) %
    colors.length

  return colors[hash] ?? colors[0]
}
