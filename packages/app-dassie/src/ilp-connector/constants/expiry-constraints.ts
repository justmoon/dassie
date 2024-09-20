/**
 * The amount of time that we reduce the expiry by per hop.
 *
 * @remarks
 *
 * Each connector in the path will look at the incoming expiry date and reduce it by this amount before forwarding.
 * This ensures that each connector has enough time to forward the fulfillment and still fulfill the incoming packet.
 *
 * Connectors will reject any incoming packet with an expiry less than twice this amount in the future. This is because
 * the connector needs to have enough time to forward the packet to the next hop and for that connector to accept
 * it since it still has one message window left.
 */
export const ILP_MESSAGE_WINDOW = 1000 // 1 second

/**
 * The maximum amount of time between packet prepare and packet fulfill.
 *
 * @remarks
 *
 * When a connector is holding a packet, it is tying up valuable resources and providing a free option on the exchange
 * rate. Therefore, it must limit both the amount and maximum time that it will hold a packet for otherwise the cost
 * would be unbounded as well.
 */
export const MAXIMUM_HOLD_TIME = 30_000 // 30 seconds
