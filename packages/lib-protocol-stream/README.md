# lib-protocol-stream

This module provides an implementation of the STREAM transport protocol of the Interledger protocol suite.

## Comparison to ilp-protocol-stream

- In ilp-protocol-stream, every connection starts with five probing packets. This is done to measure the exchange rate of the connection. However, this exchange rate cannot be trusted, since a malicious recipient can simulate any exchange rate they want. It is therefore not useful to measure it on every connection. This implementation skips this step entirely resulting in a much faster connection. Exchange rate measurement is still offered as an explicit API. This is useful for displaying the exchange rate to the user. Applications should add some slippage and then lock in the exchange rate by calculating a fixed SendMax.

- When receiving a packet where one of the streams is unable to accept the proportional amount of money allocated to it, ilp-protocol-stream will reject the whole packet. This implementation will instead try to allocate any excess funds to other streams that are still able to accept more. In the case of rounding errors, this implementation will allocate the extra amount to a stream where ilp-protocol-stream just drops it.

- This implementation uses BigInt for all numeric values. This means that numbers larger than uint64 will be correctly handled unlike ilp-protocol-stream which truncates them to uint64.

## Attribution

Implemented using [ilp-protocol-stream](https://github.com/interledgerjs/interledgerjs/tree/master/packages/ilp-protocol-stream) as the main reference. Credit to the original authors.
