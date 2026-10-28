# The DSUSD keeper follows the pool, and reprices after each trade

**Areas:** [DSUSD keeper](../areas/dsusd-keeper.md#following-the-pool), [XRPL](../areas/xrpl.md#the-peg-guard).

## Facts

- DSUSD is had only by paying XRP for it through the XRP/DSUSD pool ([dsusd](dsusd.md)), so every node that needs dollars on the testnet, and the market maker that rebalances them, buys there.
- The pool is shallow next to what nodes buy: about 1,400 XRP beside 1,800 DSUSD. Buying 40 DSUSD moves its price about 4%, and the peg guard pauses DSUSD on every node once each reading for ten minutes was more than 3% off a dollar ([XRPL](../areas/xrpl.md#the-peg-guard)).
- The keeper ran every five minutes, counted from the end of the last run, which could take up to four. After a trade, the pool stayed where the trade left it for up to nine minutes. Owners who bought dollars for a node bought a few at a time and waited for the keeper between purchases, for an hour or two for 40–50 DSUSD, and one round still came within a tenth of a percent of the guard's band.
- The XRP Ledger streams every validated transaction that touches an account to whoever subscribes to it. Every trade with the pool, and every deposit or withdrawal, touches the pool's account.
- A run is idempotent: it reads the ledger and takes the next step until the pool is at the target ([DSUSD keeper](../areas/dsusd-keeper.md#a-run)), so it may run at any time, as often as asked.

## Decision

- **The keeper follows the pool.** It subscribes to the validated transactions that touch the pool's account, and runs at once after one sent by any account but the keeper's and the issuer's. Without one, it runs once a minute, for the market's moves. Several trades in one ledger ask for one run.
- **A budget per window of time.** What a run every five minutes could spend, faucet requests, DSUSD minted and withdrawals, a window of five minutes may spend now, across its runs. So following spends no more than the timer did, however many runs the trades ask for.
- **A process lives an hour,** and systemd starts it again a few seconds after it ended, from the newest `main`, as each run did before. A process that fails ends at the end of the window it failed in, never within it, so a keeper that fails again and again spends no more than one that follows.
- **A connection lost is made again,** with a new subscription, and a run catches up on what touched the pool meanwhile.

## Accepted

- **The pool is off the market for a few ledgers after each trade:** the keeper hears of a trade once its ledger is validated, and mints and deposits in two transactions, each waiting for its own ledger. On the testnet, a trade was repriced four ledgers later, about fifteen seconds, against the ten minutes the guard waits.
- **A trade still pays its own slippage.** Following brings the pool back after a trade, not during it: buying 50 DSUSD at once from a pool this deep costs a few percent above the market. A deeper pool is a setting of the keeper's, apart from this.
- **A run per ledger while trades come every ledger,** each with a ticker request and a read of the ledger: a few requests every few seconds, on a testnet with little trading.
- **A few seconds without a keeper each hour,** and longer when the start fetches a new toolchain, while systemd starts it again.

## Rejected

- *A timer every minute:* a minute's lag after every trade, and a new process at every run, which on the keeper's VM takes longer to start than a run takes.
- *Polling the pool every ledger:* a read every few seconds whether or not anything traded, where the ledger says when something did.
- *Leaving reconnecting to xrpl.js:* it connects again by itself, but without the subscription, and the keeper would follow nothing without knowing.
- *Holding DSUSD ready between runs,* to save the mint's ledger: the keeper would hold DSUSD that neither the pool nor a buyer does, which [dsusd](dsusd.md) keeps to the pool and its buyers, to save one ledger of a few.
- *A budget per run:* with a run per ledger, a misread ticker or pool could cost each run's budget again every few seconds.
