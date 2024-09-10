import type { Promisable } from "type-fest"

import type {
  ENCRYPTION_ALGORITHMS,
  HASH_ALGORITHMS,
  MAC_ALGORITHMS,
} from "../../constants/crypto-algorithms"
import type { DecryptionFailure } from "../../failures/decryption-failure"

export type HashAlgorithm = (typeof HASH_ALGORITHMS)[number]

export type MacAlgorithm = (typeof MAC_ALGORITHMS)[number]

export type EncryptionAlgorithm = (typeof ENCRYPTION_ALGORITHMS)[number]

export type AesCryptor = Cryptor<
  Uint8Array,
  { iv: Uint8Array; tag: Uint8Array; ciphertext: Uint8Array }
>
export interface RsaKeyPair {
  getPublicKeyPem(): Promise<string>
  getPrivateKeyPem(): Promise<string>
  sign(message: Uint8Array): Promise<Uint8Array>
  verify(message: Uint8Array, signature: Uint8Array): Promise<boolean>
}

export type CryptorTypes = {
  "aes-128-gcm": AesCryptor
  "aes-256-gcm": AesCryptor
}

export interface Crypto {
  getRandomBytes(length: number): Uint8Array
  hash(algorithm: HashAlgorithm, message: Uint8Array): Promisable<Uint8Array>
  createMac(algorithm: MacAlgorithm, key: Uint8Array): HmacSigner
  createCryptor<T extends EncryptionAlgorithm>(
    algorithm: T,
    key: Uint8Array,
  ): CryptorTypes[T]
  generateRsaKeyPair(modulusLength: number): Promise<RsaKeyPair>
  importRsaKeyPair(privateKey: string | Uint8Array): Promise<RsaKeyPair>
}

export type HmacSigner = (message: Uint8Array) => Promisable<Uint8Array>

export interface Cryptor<TPlaintext = Uint8Array, TCiphertext = Uint8Array> {
  encrypt(plaintext: TPlaintext): Promisable<TCiphertext>
  decrypt(ciphertext: TCiphertext): Promisable<TPlaintext | DecryptionFailure>
}
