import { measureExchangeRate } from "./measure-exchange-rate"
import type { ConnectionState } from "./state"

export class Connection {
  constructor(private readonly state: ConnectionState) {}

  async measureExchangeRate() {
    return await measureExchangeRate({ state: this.state })
  }
}
