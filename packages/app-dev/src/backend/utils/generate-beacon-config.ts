const ENTRYPOINT = new URL("../../runner/launchers/beacon", import.meta.url)
  .pathname
const LOCAL_PATH = new URL("../../../../../local", import.meta.url).pathname

export const beaconIndexToId = (index: number) => `b${index + 1}`
export const beaconIndexToPort = (index: number) => 13_000 + index

export const generateBeaconConfig = (index: number) => {
  const id = beaconIndexToId(index)
  const port = beaconIndexToPort(index)

  return {
    id,
    port,
    config: {
      host: `${id}.localhost`,
      port: beaconIndexToPort(index),
      tlsWebCertFile: `${LOCAL_PATH}/tls/${id}.localhost/web-${id}.localhost.pem`,
      tlsWebKeyFile: `${LOCAL_PATH}/tls/${id}.localhost/web-${id}.localhost-key.pem`,
    },
    url: `https://${id}.localhost:${port}/`,
    entry: ENTRYPOINT,
  }
}
