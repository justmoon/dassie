import { effect, signal } from "@preact/signals-react"

export const walletStore = {
  seed: signal<string | undefined>(localStorage.getItem("seed") ?? undefined),
  setSeed: (seed: string) => {
    walletStore.seed.value = seed
  },
}

effect(() => {
  if (localStorage.getItem("seed") !== walletStore.seed.value) {
    if (walletStore.seed.value === undefined) {
      localStorage.removeItem("seed")
    } else {
      localStorage.setItem("seed", walletStore.seed.value)
    }
  }
})
