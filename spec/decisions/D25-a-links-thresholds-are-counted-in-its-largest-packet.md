# D25. A link's thresholds are counted in its largest packet

**Status:** active, decided 2026-10-18.
**Areas:** [Settlement](../areas/settlement.md), [Connector](../areas/connector.md)

## Facts

[D23](D23-each-node-books-its-own-fulfills.md)'s trigger and least settlement were first a tenth of the credit a node extends. Under [D20](D20-trust-follows-what-a-peer-earned-us.md) a senior extends nothing, so its trigger was nothing too: it asked only once its junior had nothing paid in advance left, after the junior's packets had already been refused for want of room. And credit grows from earnings, so thresholds tied to it would move whenever it did.

## Decision

A node counts every threshold of a link in its largest packet, not in the credit: the trigger to ask for settlement (one and a half packets), the least settlement (one), and the most in flight (two and a half, [D24](D24-money-in-flight-with-a-peer-stays-within-a-few-packets.md)). Even a senior that extends nothing then asks while its junior still has room. A node refuses to start with a default credit too small for a link to survive one crash's drift. [Settlement](../areas/settlement.md) states the numbers and the formula.

## Accepted

The trigger is below the cap on what is in flight, so a busy peer can still have a packet refused (T04) while its settlement is on its way. In the simulation that costs nothing; a trigger at the cap took so much of the credit that one crash's drift stalled a link.

## Rejected

- *A tenth of the credit:* nothing for a senior, and moving with the credit.
- *A trigger of three and a half packets, at the cap:* two triggers and two settlements took 0.9 of the 1.1 credit, and one crash at the cap stalled the link (39 of 1,200 simulated runs).
