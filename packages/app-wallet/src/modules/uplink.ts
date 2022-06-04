class Uplink {
  ws: WebSocket

  constructor() {
    this.ws = new WebSocket("wss://node1.localhost:4000")
    console.log(this.ws)
    this.ws.addEventListener("message", (event) => {
      console.log("ws message", event)
    })
  }
}

export default Uplink
