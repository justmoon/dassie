import Config from "./config"
import State from "./state"

class Context {
  constructor(readonly config: Config, readonly state: State) {}

  static async fromEnvironment() {
    const config = await Config.fromEnvironment()
    const state = await State.load(config)
    return new Context(config, state)
  }
}

export default Context
