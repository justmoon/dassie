export const useAccount = () => {
  return {
    currency: {
      symbol: "$",
      code: "USD",
      precision: 2,
      totalPrecision: 9,
    },
    balance: 10_000_000_000_000n,
  }
}
