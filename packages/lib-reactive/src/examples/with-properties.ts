import { createActor } from "../actor"
import { createReactor } from "../reactor"
import { createTopic } from "../topic"

const rootActor = () =>
  createActor((sig) => {
    sig.run(logger)
    sig.run(greeter, { toGreet: "world" })
    sig.run(greeter, { toGreet: "moms" })
  })

interface ChildProperties {
  toGreet: string
}

const outputTopic = () => createTopic<string>()

const greeter = () =>
  createActor((sig, { toGreet }: ChildProperties) => {
    sig.use(outputTopic).emit(`Hello ${toGreet}!`)
  })

const logger = () =>
  createActor((sig) => {
    sig.on(outputTopic, console.info)
  })

createReactor(rootActor)
