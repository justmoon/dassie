import { base64ToUint8Array, uint8ArrayToBase64 } from "uint8array-extras"

export const pemLabelRegex =
  /-{5}BEGIN ([\d A-Z]+)-{5}\n([\d\n+/=A-Za-z]+?)\n-{5}END ([\d A-Z]+)-{5}/

const getPemKeyHeader = (type: string) =>
  `-----BEGIN ${type.toUpperCase()}-----\n`
const getPemKeyFooter = (type: string) =>
  `\n-----END ${type.toUpperCase()}-----`

export function parsePem(pem: string) {
  const match = pemLabelRegex.exec(pem) as
    | [string, string, string, string]
    | null

  if (!match) {
    throw new Error(`No valid PEM data detected`)
  }

  const [_fullMatch, headerTag, base64Chunks, footerTag] = match

  if (headerTag !== footerTag) {
    throw new Error("Header and footer tags do not match")
  }

  const tag = headerTag

  const base64ChunksWithoutNewlines = base64Chunks.replaceAll("\n", "")

  const data = base64ToUint8Array(base64ChunksWithoutNewlines)

  return { data, tag }
}

export function serializePem(data: Uint8Array, tag: string): string {
  const base64Key = uint8ArrayToBase64(data)

  // Split the base64 into chunks of 64 characters per line for PEM formatting
  const base64Chunks = base64Key.match(/.{1,64}/g) ?? []
  const body = base64Chunks.join("\n")

  return getPemKeyHeader(tag) + body + getPemKeyFooter(tag)
}
