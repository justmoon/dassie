import create from "zustand"

interface WalletStore {
  seed: string | undefined
  setSeed: (key: string) => void
}

const getLocalStorage = (key: string) => window.localStorage.getItem(key)
const setLocalStorage = (key: string, value: string) =>
  window.localStorage.setItem(key, value)

const useWalletStore = create<WalletStore>((set) => ({
  seed: getLocalStorage("seed") ?? undefined,
  setSeed: (seed: string) =>
    set((state) => {
      setLocalStorage("seed", seed)
      return { ...state, seed: seed }
    }),
}))

export const useWallet = () => {
  const walletStore = useWalletStore()

  return {
    open: !!walletStore.seed,
    setSeed: (seed: string) => walletStore.setSeed(seed),
  }
}
