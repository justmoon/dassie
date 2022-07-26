import type { EffectContext } from "../effect"
import { createReactor } from "../reactor"
import { createTopic } from "../topic"

const rootEffect = (sig: EffectContext) => {
  sig.use(logger)
  sig.use(greeter, { toGreet: "world" })
  sig.use(greeter, { toGreet: "moms" })
}

interface ChildProperties {
  toGreet: string
}

const outputTopic = () => createTopic<string>()

const greeter = (sig: EffectContext, { toGreet }: ChildProperties) => {
  sig.emit(outputTopic, `Hello ${toGreet}!`)
}

const logger = (sig: EffectContext) => {
  sig.on(outputTopic, console.log)
}

createReactor(rootEffect)
