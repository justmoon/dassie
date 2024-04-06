export function createIdGenerator() {
  let id = 0
  return function generateId() {
    return String(id++)
  }
}
