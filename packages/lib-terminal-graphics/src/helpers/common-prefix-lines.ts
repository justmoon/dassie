export function commonPrefixLines(a: string, b: string) {
  const minLength = Math.min(a.length, b.length)
  let lineIndex = 0
  for (let index = 0; index < minLength; index++) {
    if (a[index] !== b[index]) {
      break
    }
    if (a[index] === "\n") {
      lineIndex = index + 1
    }
  }
  return lineIndex
}
