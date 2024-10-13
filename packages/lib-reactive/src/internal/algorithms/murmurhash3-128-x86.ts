function rotateLeft32(value: number, count: number): number {
  return ((value << count) | (value >>> (32 - count))) >>> 0
}

function finalizeHash32(hash: number): number {
  hash ^= hash >>> 16
  hash = Math.imul(hash, 0x85_eb_ca_6b)
  hash ^= hash >>> 13
  hash = Math.imul(hash, 0xc2_b2_ae_35)
  hash ^= hash >>> 16
  return hash >>> 0
}

/**
 * Implementation of MurmurHash3_x86_128 algorithm.
 *
 * This is a port of the C++ implementation from https://github.com/aappleby/smhasher/blob/master/src/MurmurHash3.cpp.
 */
export function murmurHash3_128_X86(
  key: ArrayBuffer | Uint8Array | string,
  seed = 0,
): Uint32Array {
  let dataBytes: Uint8Array

  if (typeof key === "string") {
    // Convert string to UTF-8 encoded Uint8Array
    dataBytes = new TextEncoder().encode(key)
  } else if (key instanceof ArrayBuffer) {
    dataBytes = new Uint8Array(key)
  } else if (key instanceof Uint8Array) {
    dataBytes = key
  } else {
    throw new TypeError("Invalid key type")
  }

  const dataLength = dataBytes.length
  const blockCount = Math.floor(dataLength / 16)

  // Initialize hash values
  let hash1 = seed >>> 0
  let hash2 = seed >>> 0
  let hash3 = seed >>> 0
  let hash4 = seed >>> 0

  // Constants
  const constant1 = 0x23_9b_96_1b >>> 0
  const constant2 = 0xab_0e_97_89 >>> 0
  const constant3 = 0x38_b3_4a_e5 >>> 0
  const constant4 = 0xa1_e3_8b_93 >>> 0

  const hash1Constant = 0x56_1c_cd_1b >>> 0
  const hash2Constant = 0x0b_ca_a7_47 >>> 0
  const hash3Constant = 0x96_cd_1c_35 >>> 0
  const hash4Constant = 0x32_ac_3b_17 >>> 0

  const dataView = new DataView(
    dataBytes.buffer,
    dataBytes.byteOffset,
    dataBytes.byteLength,
  )

  // Body: process blocks of 16 bytes
  for (let blockIndex = 0; blockIndex < blockCount; blockIndex++) {
    const index = blockIndex * 16
    let key1 = dataView.getUint32(index, true)
    let key2 = dataView.getUint32(index + 4, true)
    let key3 = dataView.getUint32(index + 8, true)
    let key4 = dataView.getUint32(index + 12, true)

    key1 = Math.imul(key1, constant1)
    key1 = rotateLeft32(key1, 15)
    key1 = Math.imul(key1, constant2)
    hash1 ^= key1
    hash1 = rotateLeft32(hash1, 19)
    hash1 = (hash1 + hash2) >>> 0
    hash1 = (Math.imul(hash1, 5) + hash1Constant) >>> 0

    key2 = Math.imul(key2, constant2)
    key2 = rotateLeft32(key2, 16)
    key2 = Math.imul(key2, constant3)
    hash2 ^= key2
    hash2 = rotateLeft32(hash2, 17)
    hash2 = (hash2 + hash3) >>> 0
    hash2 = (Math.imul(hash2, 5) + hash2Constant) >>> 0

    key3 = Math.imul(key3, constant3)
    key3 = rotateLeft32(key3, 17)
    key3 = Math.imul(key3, constant4)
    hash3 ^= key3
    hash3 = rotateLeft32(hash3, 15)
    hash3 = (hash3 + hash4) >>> 0
    hash3 = (Math.imul(hash3, 5) + hash3Constant) >>> 0

    key4 = Math.imul(key4, constant4)
    key4 = rotateLeft32(key4, 18)
    key4 = Math.imul(key4, constant1)
    hash4 ^= key4
    hash4 = rotateLeft32(hash4, 13)
    hash4 = (hash4 + hash1) >>> 0
    hash4 = (Math.imul(hash4, 5) + hash4Constant) >>> 0
  }

  // Tail: process remaining bytes
  let key1 = 0
  let key2 = 0
  let key3 = 0
  let key4 = 0

  const tailOffset = blockCount * 16
  const remainingBytes = dataLength & 15

  switch (remainingBytes) {
    case 15: {
      key4 ^= (dataBytes[tailOffset + 14]! & 0xff) << 16
    }
    // Fallthrough
    case 14: {
      key4 ^= (dataBytes[tailOffset + 13]! & 0xff) << 8
    }
    // Fallthrough
    case 13: {
      key4 ^= dataBytes[tailOffset + 12]! & 0xff
      key4 = Math.imul(key4, constant4)
      key4 = rotateLeft32(key4, 18)
      key4 = Math.imul(key4, constant1)
      hash4 ^= key4
    }
    // Fallthrough
    case 12: {
      key3 ^= (dataBytes[tailOffset + 11]! & 0xff) << 24
    }
    // Fallthrough
    case 11: {
      key3 ^= (dataBytes[tailOffset + 10]! & 0xff) << 16
    }
    // Fallthrough
    case 10: {
      key3 ^= (dataBytes[tailOffset + 9]! & 0xff) << 8
    }
    // Fallthrough
    case 9: {
      key3 ^= dataBytes[tailOffset + 8]! & 0xff
      key3 = Math.imul(key3, constant3)
      key3 = rotateLeft32(key3, 17)
      key3 = Math.imul(key3, constant4)
      hash3 ^= key3
    }
    // Fallthrough
    case 8: {
      key2 ^= (dataBytes[tailOffset + 7]! & 0xff) << 24
    }
    // Fallthrough
    case 7: {
      key2 ^= (dataBytes[tailOffset + 6]! & 0xff) << 16
    }
    // Fallthrough
    case 6: {
      key2 ^= (dataBytes[tailOffset + 5]! & 0xff) << 8
    }
    // Fallthrough
    case 5: {
      key2 ^= dataBytes[tailOffset + 4]! & 0xff
      key2 = Math.imul(key2, constant2)
      key2 = rotateLeft32(key2, 16)
      key2 = Math.imul(key2, constant3)
      hash2 ^= key2
    }
    // Fallthrough
    case 4: {
      key1 ^= (dataBytes[tailOffset + 3]! & 0xff) << 24
    }
    // Fallthrough
    case 3: {
      key1 ^= (dataBytes[tailOffset + 2]! & 0xff) << 16
    }
    // Fallthrough
    case 2: {
      key1 ^= (dataBytes[tailOffset + 1]! & 0xff) << 8
    }
    // Fallthrough
    case 1: {
      key1 ^= dataBytes[tailOffset]! & 0xff
      key1 = Math.imul(key1, constant1)
      key1 = rotateLeft32(key1, 15)
      key1 = Math.imul(key1, constant2)
      hash1 ^= key1
    }
  }

  // Finalization
  hash1 ^= dataLength
  hash2 ^= dataLength
  hash3 ^= dataLength
  hash4 ^= dataLength

  hash1 = (hash1 + hash2 + hash3 + hash4) >>> 0
  hash2 = (hash2 + hash1) >>> 0
  hash3 = (hash3 + hash1) >>> 0
  hash4 = (hash4 + hash1) >>> 0

  hash1 = finalizeHash32(hash1)
  hash2 = finalizeHash32(hash2)
  hash3 = finalizeHash32(hash3)
  hash4 = finalizeHash32(hash4)

  hash1 = (hash1 + hash2 + hash3 + hash4) >>> 0
  hash2 = (hash2 + hash1) >>> 0
  hash3 = (hash3 + hash1) >>> 0
  hash4 = (hash4 + hash1) >>> 0

  return new Uint32Array([hash1, hash2, hash3, hash4])
}
