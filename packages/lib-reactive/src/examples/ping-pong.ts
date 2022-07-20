import type { EffectContext } from "../effect"
import { createReactor } from "../reactor"
import { createTopic } from "../topic"

const pingPongTopic = () => createTopic<string>()

const pinger = (sig: EffectContext) => {
  sig.on(pingPongTopic, (message) => {
    if (message === "pong") {
      sig.timeout(() => {
        sig.emit(pingPongTopic, "ping")
      }, 75)
    }
  })
}

const ponger = (sig: EffectContext) => {
  sig.on(pingPongTopic, (message) => {
    if (message === "ping") {
      sig.timeout(() => {
        sig.emit(pingPongTopic, "pong")
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
  sig.emit(pingPongTopic, "ping")
  sig.timeout(() => void sig.reactor.dispose(), 200)
})
