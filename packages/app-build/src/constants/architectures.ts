export const SUPPORTED_ARCHITECTURES = ["x64", "arm64", "armv7l"] as const

export type Architecture = (typeof SUPPORTED_ARCHITECTURES)[number]
