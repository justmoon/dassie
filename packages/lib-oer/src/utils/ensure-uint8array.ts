export const ensureUint8Array = (input: Uint8Array | Buffer) => {
  // Check if the input is an instance of Buffer (Node.js environment)
  if (typeof Buffer !== "undefined" && Buffer.isBuffer(input)) {
    // Convert Buffer to Uint8Array using the buffer's underlying ArrayBuffer
    return new Uint8Array(input.buffer, input.byteOffset, input.byteLength)
  } else if (Object.prototype.toString.call(input) === "[object Uint8Array]") {
    // If input is already a Uint8Array, return it as-is
    return input
  } else {
    throw new TypeError("Input must be a Buffer or a Uint8Array")
  }
}
