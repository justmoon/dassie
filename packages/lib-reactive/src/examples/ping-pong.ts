import type { EffectContext } from "../effect"
import { createReactor } from "../reactor"
import { createTopic } from "../topic"

const pingPongTopic = () => createTopic<string>()

const pinger = (sig: EffectContext) => {
  sig.on(pingPongTopic, (message) => {
    if (message === "pong") {
      sig.timeout(() => {
        sig.use(pingPongTopic).emit("ping")
      }, 75)
    }
  })
}

const ponger = (sig: EffectContext) => {
  sig.on(pingPongTopic, (message) => {
    if (message === "ping") {
      sig.timeout(() => {
        sig.use(pingPongTopic).emit("pong")
      }, 75)
    }
  })
}

const logger = (sig: EffectContext) => {
  sig.on(pingPongTopic, console.log)
}

createReactor((sig) => {
  sig.run(pinger)
  sig.run(ponger)
  sig.run(logger)
  sig.use(pingPongTopic).emit("ping")
  sig.timeout(() => void sig.reactor.dispose(), 500)
})
