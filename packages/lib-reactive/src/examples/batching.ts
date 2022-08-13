import type { EffectContext } from "../effect"
import { createReactor } from "../reactor"
import { createStore } from "../store"

const topic1 = () => createStore(0)
const topic2 = () => createStore(0)
const topic3 = () => createStore(0)

const rootEffect = (sig: EffectContext) => {
  sig.interval(() => {
    // Even though we are triggering three state updates, the effect will only re-run once
    sig.emit(topic1, (a) => a + 1)
    sig.emit(topic2, (a) => a + 3)
    sig.emit(topic3, (a) => a + 5)
  }, 1000)

  sig.run((sig) => {
    const t1 = sig.get(topic1)
    const t2 = sig.get(topic2)
    const t3 = sig.get(topic3)

    console.log(`effect run with ${t1} ${t2} ${t3}`)
  })

  // Stop the application after 10 seconds
  sig.timeout(() => void sig.reactor.dispose(), 10_000)
}

createReactor(rootEffect)
