# lib-protocol-stream

This module provides an implementation of the STREAM transport protocol of the Interledger protocol suite.

## Comparison to ilp-protocol-stream

- This implementation uses BigInt for all numeric values. This means that numbers larger than uint64 will be correctly handled unlike ilp-protocol-stream which truncates them to uint64.

## Attribution

Implemented using [ilp-protocol-stream](https://github.com/interledgerjs/interledgerjs/tree/master/packages/ilp-protocol-stream) as the main reference. Credit to the original authors.
