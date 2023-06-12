import { createActor } from "../actor"
import { createReactor } from "../reactor"
import { createTopic } from "../topic"

const pingPongTopic = () => createTopic<string>()

const pinger = () =>
  createActor((sig) => {
    sig.on(pingPongTopic, (message) => {
      if (message === "pong") {
        sig.timeout(() => {
          sig.use(pingPongTopic).emit("ping")
        }, 75)
      }
    })
  })

const ponger = () =>
  createActor((sig) => {
    sig.on(pingPongTopic, (message) => {
      if (message === "ping") {
        sig.timeout(() => {
          sig.use(pingPongTopic).emit("pong")
        }, 75)
      }
    })
  })

const logger = () =>
  createActor((sig) => {
    sig.on(pingPongTopic, console.info)
  })

const rootActor = () =>
  createActor((sig) => {
    sig.run(pinger)
    sig.run(ponger)
    sig.run(logger)
    sig.use(pingPongTopic).emit("ping")
    sig.timeout(() => void sig.reactor.dispose(), 500)
  })

createReactor(rootActor)
