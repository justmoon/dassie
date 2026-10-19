# D25. A link's thresholds are counted in its largest packet

**Areas:** [Settlement](../areas/settlement.md), [Connector](../areas/connector.md)

## Facts

Thresholds counted as a share of the credit fail twice: a senior extends nothing ([D20](D20-trust-follows-what-a-peer-earned-us.md)), so its trigger to ask for settlement would be nothing too, and it would ask only after its junior's packets had already been refused for want of room; and credit grows from earnings, so the thresholds would move whenever it did.

## Decision

A node counts every threshold of a link in its largest packet, not in the credit: the trigger to ask for settlement, the least settlement, and the most in flight ([D24](D24-money-in-flight-with-a-peer-stays-within-a-few-packets.md)). Even a senior that extends nothing then asks while its junior still has room. A node refuses to start with a default credit too small for a link to survive one crash's drift. [Settlement](../areas/settlement.md) states the numbers and the argument for them.

## Accepted

The trigger is below the cap on what is in flight, so a busy peer can still have a packet refused (T04) while its settlement is on its way. In the simulation that costs nothing.

## Rejected

- *A share of the credit:* nothing for a senior, and moving with the credit.
- *A trigger at the cap on what is in flight:* two triggers and two settlements took most of the default credit, and in the simulation one crash at the cap stalled the link in a few percent of runs.
