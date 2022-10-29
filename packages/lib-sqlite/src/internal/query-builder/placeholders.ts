export interface PlaceholderStore {
  assign(value: unknown): string
  get(): Record<string, unknown>
}

export const createPlaceholderStore = (): PlaceholderStore => {
  const placeholders: Record<string, unknown> = {}
  const nextPlaceholderId = 0

  return {
    assign(value: unknown) {
      const placeholderId = `p${nextPlaceholderId}`
      placeholders[placeholderId] = value
      return placeholderId
    },

    get() {
      return placeholders
    },
  }
}
