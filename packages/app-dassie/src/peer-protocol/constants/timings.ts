/**
 * Maximum time (in milliseconds) a node will wait before forcing an update to
 * its link state.
 *
 * @remarks
 *
 * If this amount of time passes without changes to a node's link state, it will
 * update the sequence number and publish a new link state anyway. This is to
 * show that it is still alive.
 */
export const LINK_STATE_MAX_UPDATE_INTERVAL = BigInt(3600 * 1000) // 1 hour

/**
 * Maximum time (in milliseconds) a node will wait to retransmit a link state
 * update.
 *
 * @remarks
 *
 * When receiving a new link state update for the first time, a node will pick a
 * random delay between 0 and this value to wait before retransmitting the update.
 *
 * If more than a certain number of updates are received before the delay expires,
 * the node will not retransmit the update.
 */
export const LINK_STATE_MAX_UPDATE_RETRANSMIT_DELAY = 500 // 500 ms

/**
 * How often (in milliseconds) a node will query its bootstrap nodes for their
 * latest node list hash.
 */
export const NODE_LIST_HASH_POLLING_INTERVAL =
  import.meta.env.DEV ?
    1000 // 1 sec during development
  : 60 * 1000 // 1 min in production

/**
 * Timeout when query node list hash from a bootstrap node.
 */
export const NODE_LIST_HASH_TIMEOUT = 5000 // 5 sec

/**
 * Default timeout for communicating with other nodes.
 */
export const DEFAULT_NODE_COMMUNICATION_TIMEOUT = 30_000 // 30 sec

/**
 * Time after which node state will be considered stale and a new link state
 * will be actively sought.
 *
 * @remarks
 *
 * Should be significantly higher than {@link LINK_STATE_MAX_UPDATE_INTERVAL} to
 * give new link states time to propagate.
 */
export const NODE_STATE_STALE_TIMEOUT = 3 * 3_600_000 // 3 hours

/**
 * Minimum time (in milliseconds) a node will wait before sending a heartbeat to
 * each of its peers.
 */
export const MIN_HEARTBEAT_INTERVAL = 5000

/**
 * Maximum time (in milliseconds) a node will wait before sending a heartbeat to
 * each of its peers.
 */
export const MAX_HEARTBEAT_INTERVAL = 20_000
