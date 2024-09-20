export const compareUint8Arrays = (a: Uint8Array, b: Uint8Array): boolean => {
  if (a.length !== b.length) {
    return false
  }

  for (let index = 0, length = a.length; index < length; index++) {
    if (a[index] !== b[index]) {
      return false
    }
  }

  return true
}
