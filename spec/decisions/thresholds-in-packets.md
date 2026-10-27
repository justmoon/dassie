# A link's thresholds are counted in its largest packet

**Areas:** [Settlement](../areas/settlement.md), [Connector](../areas/connector.md)

## Facts

Thresholds counted as a share of the credit fail twice: a node may extend nothing, as a payee over a hub that holds nothing toward its payer does ([Settlement](../areas/settlement.md#credit)), so its trigger to ask for settlement would be nothing too, and it would ask only after its peer's packets had already been refused for want of room; and over a hub the credit follows the holds, so the thresholds would move whenever they did.

## Decision

A node counts every threshold of a link in its largest packet, not in the credit: the trigger to ask for settlement, the least settlement, and the most in flight ([packets-in-flight](packets-in-flight.md)). Even a node that extends nothing then asks while its peer still has room. A node refuses to start with a default credit too small for a request to settle to fund at least the least settlement. [Settlement](../areas/settlement.md) states the numbers and the argument for them.

## Accepted

The trigger is below the cap on what is in flight, so a busy peer can still have a packet refused (T04) while its settlement is on its way. In the simulation that costs nothing.

## Rejected

- *A share of the credit:* nothing for a node that extends nothing, and moving with the credit.
- *A trigger at the cap on what is in flight:* two triggers and two settlements took most of the default credit, and in the simulation one crash at the cap stalled the link in a few percent of runs.
