import { createReactor } from "../create-reactor"
import { createTopic } from "../create-topic"
import type { EffectContext } from "../use-effect"

const pingPongTopic = createTopic<string>("pingPongTopic")

const pinger = (sig: EffectContext) => {
  sig.on(pingPongTopic, (message) => {
    if (message === "pong") {
      sig.reactor.emit(pingPongTopic, "ping")
    }
  })
}

const ponger = (sig: EffectContext) => {
  sig.on(pingPongTopic, (message) => {
    if (message === "ping") {
      sig.timeout(() => {
        sig.reactor.emit(pingPongTopic, "pong")
      }, 75)
    }
  })
}

const logger = (sig: EffectContext) => {
  sig.on(pingPongTopic, console.log)
}

createReactor((sig: EffectContext) => {
  sig.use(pinger)
  sig.use(ponger)
  sig.use(logger)
  sig.reactor.emit(pingPongTopic, "ping")
  sig.timeout(() => void sig.reactor.dispose(), 200)
})
