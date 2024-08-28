export interface CryptoContext {
  getRandomBytes(length: number): Uint8Array
  hash(message: Uint8Array): Promise<Uint8Array>
  createHmac(key: Uint8Array): HmacSigner
  createAesCryptor(key: Uint8Array): Cryptor
}

export type HmacSigner = (message: Uint8Array) => Promise<Uint8Array>

export interface Cryptor {
  encrypt(plaintext: Uint8Array): Promise<Uint8Array>
  decrypt(ciphertext: Uint8Array): Promise<Uint8Array>
}
