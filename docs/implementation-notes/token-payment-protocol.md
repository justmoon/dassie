# Implementation Notes: Token Payment Protocol

## Problem

Suppose node B pays a given network participant node A for a service such as including node B's advertisement in node A's cryptographically signed weekly publication. It would be nice if node B would have some proof of node A's dishonesty if node A accepts the payment but does not perform the service (including the ad).

## Naive Solution (tiny payments only)

When node B queries node A about the price for the service, the resulting quote contains an executionCondition, an expiry date, and a signature from node A over the whole quote. When node B makes the paid request, its payment ILP packet will use the given executionCondition. If node A takes the money but does not include node B in the list, then node B will publish a proof consisting of:
  1. The quote
  2. The fulfillment corresponding to the execution condition
  3. The signed weekly publication which does not include node B's advert

Anyone seeing this proof can then stop dealing with node A forever, thereby punishing node A for its dishonesty.

Unfortunately, this scheme would only work if the price of the service is less than the maximum size of one ILP packet.

## Improved Solution (larger payments are possible)

### Approach

In this improved scheme, the payment process is split into two parts. Before the payment, node B still requests a signed quote from node A. However, this time, the price is quoted in terms of a number of "tokens" issued by node A. Node B then proceeds with the payment in two phases:

1. Node B purchases the required number of tokens.
2. Node B purchases the service using the tokens.

When purchasing tokens from node A, node B can use the same protocol as before because the value of the tokens is chosen such that each token can be purchased with a single ILP packet. That means that if node A is dishonest during this process, node B has proof.

Eventually, node B submits a request to redeem the tokens for the service, referencing the quote. Node A responds with a signed acknowledgement. If node A does not include perform the service, node B simply provides the signed acknowledgement and some proof that the service was not performed (such as the signed weekly publication without the ad in it).

### Attack Vector: Head in the sand

Of course, node A could simply refuse to redeem the tokens for the service. To prove this, node B might submit a valid request via a set of witnesses. I.e. node B generates a valid request that is well within the required deadline and contains the correct amount of valid tokens. It sends this request to witnesses W_1, W_2, ..., W_n who each forward it to node A. If node A still does not honor the request, it has to claim that node B and all witnesses are colluding against it.

### Attack Vector: False double spend

Another attack is if node A claims that the token were already spent when B made its request. This would be easy for A to prove however. It would simply need to present the transaction where the tokens were spent. Since spending tokens requires a signature from the token holder, node A could not forge such transactions.  

In other words, we can prevent this attack if the witnesses sign not just the request they forwarded to node A but also how node A responded. If node A responds by refusing the transaction but presents a valid proof of double spend for one of the tokens, then this does not count as dishonesty on the part of node A. But if node A responds with no valid reason for denying the transaction or doesn't respond at all, it does count as potential dishonest behavior.

### Attack Vector: Closed for today

Yet another way that node A could attempt to defraud node B is by initially selling it some number of tokens but then refusing to sell enough tokens to purchase the service node B wants to purchase. Technically, node A hasn't done anything dishonest, but node B is still out some money without anything to show for it. Fortunately, this might be quite tricky to do. If node A is doing a decent amount of business, it might have a hard time distinguishing node B's token purchases from those of other customers. All tokens cost the same and ILP packet headers don't contain a sender address. Node B could use a unique public key for each token purchase.

### Attack Vector: Denial of service

Node A could be made to look dishonest if it comes under a denial of service attack during the submission phase of the protocol. If the witnesses are trying to contact node A but can't get through because node A is under attack, it may look like node A is maliciously not responding.

In order to mitigate this, all of the usual DoS mitigations could be used, which are beyond the scope of this article. In addition, generous deadlines help because it would require the attacker to keep the attack going for longer. For instance, if the deadline to submit the request is after one week, the witnesses could continue to retry the request every hour for a week and eventually might get through.

### Complete Protocol

It is November 1.

1. Node B requests a quote from Node A. In its request, it includes a description of the service requested (e.g. inclusion of the phrase "foobar" in the November 4th issue of Node A's weekly magazine.)
2. Node A responds with a quote containing:
   - The description of the service
   - The price of the service in tokens
   - An expiry date for the quote (e.g. November 2 at noon)
   - A signature over the entire quote (except the signature itself)
3. Node B then sends an unfulfillable packet to node containing some amount which it guesses is enough to buy a token.
4. Node A responds with a rejection containing information about the actual price of the tokens relative to the amount from the original packet.
5. Node B uses the information from the rejection to estimate the cost per token. It multiplies the cost per token with the number of tokens to get the total cost of the service. This is where the cost might be displayed to the user and the user would choose to purchase the service or not.
6. Node B sends a request for a quote to purchase a token, including a unique public key.
7. Node A responds with a signed quote containing:
   - That the quote is about purchasing one token
   - The unique public key node B provided
   - An executionCondition
8. Node B sends an amount sufficient to purchase one token using the executionCondition from the quote.
9. Node A fulfills the packet thereby making the token valid.
10. Node B repeats steps 6-9 until it has a sufficient number of tokens per the quote from step 2.
11. Node B makes a request to node A to redeem the tokens for the service.
12. Node A responds with a signed acknowledgement.
13. If Node B receives a signed acknowledgement, stop. If not, continue:
14. Node B submits its request to a set of witnesses W.
15. Each witness submits the request to node A, records the response, and return a witness report to node B containing:
    - The identity of the witness
    - The current date and time
    - The URL the request was submitted to
    - The request itself
    - The response received
    - A signature over the whole witness report (except for the signature itself)

