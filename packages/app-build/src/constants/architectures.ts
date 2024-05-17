export const SUPPORTED_ARCHITECTURES = [
  "x64",
  // TODO: Re-enable ARM build once better-sqlite3 v10 binaries are available
  //"arm64",
  //"armv7l",
] as const

export type Architecture = (typeof SUPPORTED_ARCHITECTURES)[number]
