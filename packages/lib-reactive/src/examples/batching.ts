import type { EffectContext } from "../effect"
import { createReactor } from "../reactor"
import { createStore } from "../store"

const store1 = () => createStore(0)
const store2 = () => createStore(0)
const store3 = () => createStore(0)

const rootEffect = (sig: EffectContext) => {
  sig.interval(() => {
    // Even though we are triggering three state updates, the effect will only re-run once
    sig.use(store1).update((a) => a + 1)
    sig.use(store2).update((a) => a + 3)
    sig.use(store3).update((a) => a + 5)
  }, 1000)

  sig.run((sig) => {
    const t1 = sig.get(store1)
    const t2 = sig.get(store2)
    const t3 = sig.get(store3)

    console.log(`effect run with ${t1} ${t2} ${t3}`)
  })

  // Stop the application after 10 seconds
  sig.timeout(() => void sig.reactor.dispose(), 10_000)
}

createReactor(rootEffect)
