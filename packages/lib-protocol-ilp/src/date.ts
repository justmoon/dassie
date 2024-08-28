function pad(n: number) {
  return n < 10 ? "0" + String(n) : String(n)
}

export const timestampToInterledgerTime = (timestamp: number) => {
  const date = new Date(timestamp)
  return (
    String(date.getUTCFullYear()) +
    pad(date.getUTCMonth() + 1) +
    pad(date.getUTCDate()) +
    pad(date.getUTCHours()) +
    pad(date.getUTCMinutes()) +
    pad(date.getUTCSeconds()) +
    (date.getUTCMilliseconds() / 1000).toFixed(3).slice(2, 5)
  )
}

export const INTERLEDGER_TIME_LENGTH = 17

export const interledgerTimeToTimestamp = (interledgerTime: string): number => {
  if (interledgerTime.length !== INTERLEDGER_TIME_LENGTH) {
    throw new Error("Invalid interledger time")
  }

  const date = Date.UTC(
    +interledgerTime.slice(0, 4), // year
    +interledgerTime.slice(4, 6) - 1, // month
    +interledgerTime.slice(6, 8), // day
    +interledgerTime.slice(8, 10), // hours
    +interledgerTime.slice(10, 12), // minutes
    +interledgerTime.slice(12, 14), // seconds
    +interledgerTime.slice(14, 17), // milliseconds
  )

  return date
}
