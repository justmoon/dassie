// See: https://stackoverflow.com/questions/40031688/javascript-arraybuffer-to-hex
const byteToHex = Array.from({ length: 256 }).map((_, index) =>
  index.toString(16).padStart(2, "0"),
)

export const uint8ArrayToHex = (
  uint8Array: Uint8Array,
  byteSeparator = " ",
) => {
  const hex = []
  for (const element of uint8Array) {
    hex.push(byteToHex[element])
  }
  return hex.join(byteSeparator)
}

export const hexToUint8Array = (hexString: string) => {
  hexString = hexString.replaceAll(/[^\dA-Fa-f]/g, "")

  if (hexString.length % 2 !== 0) {
    throw new Error("hexString must have an even number of characters")
  }

  const uint8Array = new Uint8Array(hexString.length / 2)
  for (let index = 0; index < hexString.length; index += 2) {
    uint8Array[index / 2] = Number.parseInt(
      hexString.slice(index, index + 2),
      16,
    )
  }
  return uint8Array
}
