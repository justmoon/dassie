import create from "zustand"
import { persist } from "zustand/middleware"

import type { AccountConfiguration } from "../types/account-configuration"

interface WalletStore {
  seed: string | undefined
  accounts: AccountConfiguration[]

  setSeed: (key: string) => void
  addSubnet: (subnet: AccountConfiguration) => void
}

export const useWallet = create<WalletStore>()(
  persist(
    (set) => ({
      seed: undefined,
      accounts: [],

      setSeed: (seed: string) => set({ seed }),
      addSubnet: (subnet: AccountConfiguration) => {
        set((state) => ({ accounts: [...state.accounts, subnet] }))
      },
    }),
    {
      name: "wallet",
    }
  )
)
