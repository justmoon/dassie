import type { EffectContext } from "../effect"
import { createReactor } from "../reactor"
import { createStore } from "../store"

const counterStore = () => createStore(0)

const rootEffect = (sig: EffectContext) => {
  sig.interval(() => {
    sig.emit(counterStore, (a) => a + 1)
  }, 500)

  void sig.use(innerEffect)
}

const innerEffect = async (sig: EffectContext) => {
  const counter = sig.get(counterStore)

  // This will only print every three seconds or so
  console.log(counter)

  // Wait long enough so the counter will have updated a bunch of times before we're ready to run again
  await new Promise((resolve) => setTimeout(resolve, 3000))
}

createReactor(rootEffect)
