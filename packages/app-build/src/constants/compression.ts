export const SUPPORTED_COMPRESSIONS = ["gz", "xz"] as const

export type Compression = (typeof SUPPORTED_COMPRESSIONS)[number]
