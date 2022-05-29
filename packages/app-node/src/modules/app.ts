import Context from "./context"

class App {
  constructor(private context: Context) {}

  async start() {
    setTimeout(() => {}, 0x7fffffff)
    console.log("app is launched")
  }

  static async launch() {
    const context = await Context.fromEnvironment()
    const app = new App(context)
    console.log(context.config.data.dataPath)
    return app.start()
  }
}

export default App
