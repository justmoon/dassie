# The event log is a file the node writes, opt-in, bounded and never waited for

**Areas:** [Node](../areas/node.md#the-event-log), [RPC](../areas/rpc.md#live-streams), [Connector](../areas/connector.md#the-packet-trace), [Settlement hubs](../areas/hubs.md#in-the-node).

## Facts

- `SubscribePackets` and `SubscribeHubEvents` stream the packet trace and the hub events live, through bounded broadcasts: a subscriber that falls behind or is not connected misses events for good, since the engine buffers nothing for it ([RPC](../areas/rpc.md#live-streams)).
- Watching a network means reading what many nodes saw of the same packets, joined by condition across hops ([Connector](../areas/connector.md#the-packet-trace)), often after the fact and from nodes other operators run.
- Each node stamps its events with its own clock, which is wall-clock time.
- The packet path must never wait on disk, and every queue and file is bounded ([tigerstyle](tigerstyle.md)).
- The records carry amounts, addresses, peers and conditions: what any observer of the node's traffic would want to keep from others.

## Decision

The node itself appends the events of both streams to a file the owner names in `DASSIE_EVENT_LOG`, one JSON object per line, converted by the RPC's own functions, through a bounded queue to a writer thread, with a `lost` line counting what was dropped, and rotated at a size cap with a fixed number of older files kept. It is off unless the owner sets the variable, and a file the node cannot open stops it at start.

## Accepted

- **The node does disk work it would not otherwise do,** off the engine's step, on a thread of its own, at most a queue's worth of lines behind.
- **While the log is on, the engine makes the hub events,** as it does while an RPC client listens, and the log takes one of the hub stream's places.
- **A second format beside the proto's:** the lines follow proto JSON names, so a script that reads both maps nothing by hand, but bytes are hex and defaults are written. The golden lines in the tests pin it.
- **A start refusal for a path the node cannot write.** A node that ran without the record the owner asked for would show the gap only when the record is read.

## Rejected

- *A recorder that subscribes over RPC and writes the file.* It loses whatever happens while it is disconnected, restarting or behind, which the stream cannot replay; it needs the owner's credentials and a process beside every node; and collecting from many operators' nodes means asking each of them to run one. A file the node writes survives a reader's absence and is collected after the fact.
- *Logging the events through `tracing`.* The journal would carry every packet, at whatever level, mixed with the node's own lines and bounded by the journal's retention rather than the owner's cap, and turning it on would mean turning up a log level that also shows other things.
- *On by default, or turned on by setup or the installer.* It records other people's payments through the node, so only an owner who chose to record their own node's traffic should have it.
- *Blocking when the queue is full,* or an unbounded queue: either makes the packet path or the node's memory depend on the disk.
