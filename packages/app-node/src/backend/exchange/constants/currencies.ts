export const CURRENCIES = {
  USD: {
    code: "USD",
    scale: 9,
  },
  XRP: {
    code: "XRP",
    scale: 9,
  },
} as const

export type CurrencyId = keyof typeof CURRENCIES
