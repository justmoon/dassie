const ENTRYPOINT = new URL("../../runner/launchers/node", import.meta.url)
  .pathname
const LOCAL_PATH = new URL("../../../../../local", import.meta.url).pathname

export const nodeIndexToId = (index: number) => `n${index + 1}`
export const nodeIndexToPort = (index: number) => 4000 + index

export const generateNodeConfig = (index: number, peers: readonly number[]) => {
  const id = nodeIndexToId(index)
  const port = nodeIndexToPort(index)

  return {
    id,
    port,
    debugPort: port + 1000,
    peers: peers.map((index) => nodeIndexToId(index)),
    config: {
      nodeId: id,
      subnetId: "xrp",
      host: `${id}.localhost`,
      port: nodeIndexToPort(index),
      tlsDassieCertFile: `${LOCAL_PATH}/ssl/${id}.localhost/dassie-${id}.localhost.pem`,
      tlsDassieKeyFile: `${LOCAL_PATH}/ssl/${id}.localhost/dassie-${id}.localhost-key.pem`,
      tlsWebCertFile: `${LOCAL_PATH}/ssl/${id}.localhost/web-${id}.localhost.pem`,
      tlsWebKeyFile: `${LOCAL_PATH}/ssl/${id}.localhost/web-${id}.localhost-key.pem`,
      initialPeers: peers
        .map(
          (target) =>
            `${nodeIndexToId(target)}=https://${nodeIndexToId(
              target
            )}.localhost:${nodeIndexToPort(target)}`
        )
        .join(";"),
    },
    url: `https://${id}.localhost:${port}/`,
    entry: ENTRYPOINT,
  }
}