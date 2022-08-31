import { createSynchronizableStore } from "@dassie/lib-reactive"

import type { AccountConfiguration } from "../types/account-configuration"

export interface WalletModel {
  version: 1
  seed: string | undefined
  accounts: AccountConfiguration[]
}

const loadWallet = (): WalletModel => {
  try {
    const storedWallet = localStorage.getItem("wallet")

    if (storedWallet) {
      const parsedWallet = JSON.parse(storedWallet) as unknown

      return parsedWallet as WalletModel
    }
  } catch (error) {
    console.error("failed to load wallet", error)
  }

  return {
    version: 1,
    seed: undefined,
    accounts: [],
  }
}

export const walletStore = () =>
  createSynchronizableStore(loadWallet(), {
    setSeed: (seed: string) => (state) => ({ ...state, seed }),
    addAccount: (subnet: AccountConfiguration) => (state) => ({
      ...state,
      accounts: [...state.accounts, subnet],
    }),
    addUplink:
      <T extends AccountConfiguration>(
        targetAccount: T,
        uplink: T["uplinks"][number]
      ) =>
      (state: WalletModel) => ({
        ...state,
        accounts: state.accounts.map((account) => {
          if (account === targetAccount) {
            return { ...account, uplinks: [...account.uplinks, uplink] }
          }

          return account
        }),
      }),
  })
