export interface CurrencySpecification {
  /**
   * Currency symbol.
   *
   * This will be used to denote the currency in the UI, next balances or amounts.
   *
   * @example "$"
   * @example "kn"
   * @example "RD$"
   */
  symbol: string

  /**
   * ISO 4217 currency code.
   *
   * @example "USD"
   */
  code: string

  /**
   * Number of decimal places to display.
   */
  precision: number

  /**
   * Location of the decimal point when expressing amounts in this currency as a bigint.
   */
  totalPrecision: number
}
