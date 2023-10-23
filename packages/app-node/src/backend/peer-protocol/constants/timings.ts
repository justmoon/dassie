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
export const LINK_STATE_MAX_UPDATE_INTERVAL = BigInt(48 * 3600 * 1000) // 48 hours

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
export const LINK_STATE_MAX_UPDATE_RETRANSMIT_DELAY = 500
