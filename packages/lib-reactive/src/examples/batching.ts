import { createReactor } from "../create-reactor"
import { createTopic } from "../create-topic"
import type { EffectContext } from "../use-effect"

const topic1 = createTopic<string>("topic1")
const topic2 = createTopic<string>("topic2")
const topic3 = createTopic<string>("topic3")

const rootEffect = (sig: EffectContext) => {
  console.log("root effect created")
  let count = 0
  sig.interval(() => {
    console.log(" ")
    // Even though we are triggering two state updates, the effect will only re-render once
    sig.reactor.emit(topic1, "hello" + String(count))
    sig.reactor.emit(topic2, "world" + String(count * 3))
    sig.reactor.emit(topic3, "!" + String(count * 5))
    count++
  }, 1000)

  sig.use((sig) => {
    console.log("child effect created")
    const t1 = sig.get(topic1)
    const t2 = sig.get(topic2)
    const t3 = sig.get(topic3)

    console.log("topic time", t1, t2, t3)

    sig.onCleanup(() => {
      console.log("child effect cleaned up")
    })
  })

  sig.onCleanup(() => {
    console.log("root effect cleaned up")
  })
}

const { dispose: stopApplication } = createReactor(rootEffect)

setTimeout(() => void stopApplication(), 10_000)
