import { createActor } from "../actor"
import { createReactor } from "../reactor"
import { createTopic } from "../topic"

const RootActor = () =>
  createActor((sig) => {
    sig.run(LoggerActor)
    sig.run(GreeterActor, { toGreet: "world" })
    sig.run(GreeterActor, { toGreet: "moms" })
  })

interface ChildProperties {
  toGreet: string
}

const OutputTopic = () => createTopic<string>()

const GreeterActor = () =>
  createActor((sig, { toGreet }: ChildProperties) => {
    sig.use(OutputTopic).emit(`Hello ${toGreet}!`)
  })

const LoggerActor = () =>
  createActor((sig) => {
    sig.on(OutputTopic, console.info)
  })

createReactor(RootActor)
