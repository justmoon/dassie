import create from "zustand"
import { persist } from "zustand/middleware"

import type { SubnetConfiguration } from "../types/subnet-configuration"

interface WalletStore {
  seed: string | undefined
  subnets: SubnetConfiguration[]

  setSeed: (key: string) => void
  addSubnet: (subnet: SubnetConfiguration) => void
}

export const useWallet = create<WalletStore>()(
  persist(
    (set) => ({
      seed: undefined,
      subnets: [],

      setSeed: (seed: string) => set({ seed }),
      addSubnet: (subnet: SubnetConfiguration) => {
        set((state) => ({ subnets: [...state.subnets, subnet] }))
      },
    }),
    {
      name: "wallet",
    }
  )
)
