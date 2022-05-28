import reduct from "reduct"
import type { Injector } from "reduct"
import Config from "./config"

class App {
  private config: Config

  constructor(deps: Injector) {
    this.config = deps(Config)
  }

  async start() {
    setTimeout(() => {}, 0x7fffffff)
    console.log("app is launched")
  }

  static async launch() {
    const deps = reduct()
    const app = new App(deps)
    return app.start()
  }
}

export default App
