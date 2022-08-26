export default class NullSubnet {
  getBalance() {
    return Promise.resolve(0)
  }
}
