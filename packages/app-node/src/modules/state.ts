import Config from "./config"

class State {
  static async load(config: Config) {
    return new State()
  }
}

export default State
