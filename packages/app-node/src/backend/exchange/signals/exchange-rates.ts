import { createSignal } from "@dassie/lib-reactive"

export interface ExchangeRates {
  readonly baseCurrency: string
  readonly rates: Record<string, bigint>
}
export const ExchangeRatesSignal = () =>
  createSignal<ExchangeRates>({
    baseCurrency: "USD",
    rates: {},
  })
