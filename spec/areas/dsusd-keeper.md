# DSUSD keeper

DSUSD is the testnet dollar token of our own that nodes settle in, the testnet's stand-in for RLUSD, and that owners get only by paying XRP for it through the XRP Ledger testnet's XRP/DSUSD pool ([dsusd](../decisions/dsusd.md)). The keeper is a small program, on a VM of its own, that keeps the token's issuer, its own account and the pool in place, and holds the pool at an exchange's XRP/USD price: it follows the pool, and brings it back within a few ledgers of each trade.

**Paths:** `apps/dsusd-keeper/`
**Read with:** [XRPL](xrpl.md) (tokens, trust lines, the testnet), [Testing](testing.md)

It is tooling ([engineering](../engineering.md#where-the-rules-bend)): TypeScript run by Node, with xrpl.js, pinned exactly, as its one dependency. It is still bounded and exact: amounts are integers (drops, and DSUSD in millionths, as the node counts a dollar token), never floats; every request has a timeout, every wait and every loop a bound; and what it reads is parsed into error values before it acts on anything.

## The token

- **Two accounts, each from a seed of its own:** the issuer, whose address is what DSUSD is named by, and the keeper, which holds the pool's LP tokens and does all the trading. The issuer only sets its flags, mints to the keeper and takes DSUSD back. The issuer's seed is made once and never changes: nodes name DSUSD by its address.
- **The code is `DSUSD`,** five letters, which the ledger takes only as 40 hex digits: the letters in ASCII, padded with zeros (`DSUSD_CURRENCY` in `src/units.ts`).
- **The issuer sets DefaultRipple,** which a pool of its token needs. The keeper's line to it has NoRipple on the keeper's side and the ledger's largest limit, so no payment ripples through the keeper and no mint fails for the limit.
- **The pool is XRP/DSUSD,** made by the keeper with a trading fee of its own (`TRADING_FEE_UNITS`), at the target price.
- **What is outstanding** is what the pool and the people who bought DSUSD hold: the keeper gives back to the issuer any DSUSD it is left holding at the end of a run.

## A run

A run remembers nothing of the runs before it. It reads the exchange's ticker, then reads the ledger and takes one step, and reads again, until the plan is done (`src/plan.ts`):

1. Fund the issuer and the keeper from the faucet if either is missing, which is also how a testnet reset is met.
2. Set DefaultRipple on the issuer, and the keeper's line.
3. Make the pool if there is none, or fill one emptied, at the target, with as much of the configured depth as the keeper's XRP allows.
4. Move the pool's price to the target, if it is outside the band ([below](#repricing-with-dsusd-alone)).
5. Deepen the pool toward the configured depth, with faucet XRP and DSUSD beside it at the pool's ratio.
6. Give back what DSUSD the keeper holds.

DSUSD the keeper needs for a step is minted to it first, as a step of its own. Each step is decided from what the ledger shows as validated, so a run that stopped halfway, or one after a reset, starts from what is there; the same state gives the same step.

**A run's bounds** are a number of steps, a time, and what its window may still spend (`BUDGET`, [below](#following-the-pool)): faucet requests, DSUSD minted, and withdrawals. A run that meets a bound, or fails, or cannot do what it must (it is stuck), says why; the next run starts again from the ledger. The faucet is best effort: when it fails, the run goes on without it, and is stuck only if it needed the XRP. A run's line holds the target, the pool after the run, its steps, and the two addresses.

**Only on the testnet.** The keeper refuses a server that does not report the testnet's network ID.

`--once` runs once with a full budget, prints its line and exits; `--dry-run` prints the first step of a run and submits nothing.

## Following the pool

The keeper is one process that runs the plan whenever the pool may be off the target, for a bounded life ([keeper-follows-the-pool](../decisions/keeper-follows-the-pool.md), `src/follow.ts`):

- **After a trade.** It subscribes to the validated transactions that touch the pool's account, as the last run read it, and runs at once after one sent by any account but the keeper's and the issuer's, whose transactions leave the pool where their run put it. A transaction of a ledger a run has already read, as the second of several trades in one ledger, asks for no run. A testnet reset makes the pool at another account, which it follows once a run made it.
- **For the market's moves,** it runs once a minute without a trade.
- **A budget per window.** Each window of five minutes, counted from the process's start, has the budget one run had when the keeper ran every five minutes, and its runs spend it together. A run spends from the window it started in, never the next one's.
- **A bounded life.** The process ends after an hour of windows, and systemd starts it again a few seconds later, from the newest `main` ([below](#its-vm)). A connection lost is made again, with its subscription, every few seconds until it is, and a run catches up on what happened meanwhile. A run that fails or is stuck is said once, however often it repeats, and the next trigger runs again. Anything else the process does not expect, such as a run with no end, ends it at the end of the window it happened in, never within one, so the next process's first window shares no time with one of this one's.
- **What it prints:** a line for each run that took a step, one for each failure or stuck run that differs from the last, and one for each window, with its runs, its steps and the last run's line.

## The price

The target is the mid between the bid and the ask of one exchange's XRP/USD ticker, in millionths of a dollar ([dsusd](../decisions/dsusd.md)). Before it moves any money the keeper refuses a ticker that is stale or from the future, crossed, wider than a bound, or outside the prices it believes (`src/price.ts`): moving the pool on a broken price would mint or burn DSUSD for nothing.

The exchange's terms forbid publishing its price on a pool; a target from mainnet's XRP/RLUSD pool instead is not built yet ([roadmap](../roadmap.md#open-rate-sources)).

## Repricing with DSUSD alone

A single-asset deposit or withdrawal of DSUSD moves the pool's DSUSD by exactly its amount and leaves its XRP as it is; the trading fee is taken in the LP tokens it issues or burns. So the amount to move is the difference between the pool's DSUSD and what its XRP is worth at the target, exactly (`gap` in `src/amm.ts`), and the pool lands on the target. Within the band (`DSUSD_KEEPER_BAND_PPM`) the keeper leaves it alone.

- **XRP too cheap:** the keeper deposits newly minted DSUSD.
- **XRP too dear:** the keeper withdraws DSUSD, and gives it back to the issuer. It withdraws at most its share of the pool's DSUSD, which never needs more LP tokens than it holds, at any trading fee (the argument is at `withdrawCap`); and once a window at most, since with a small share each withdrawal sells the rest of its XRP in the pool to the other providers. A keeper that cannot lower the price says so, and the run is stuck.
- **It never buys or sells XRP.** No step pays the keeper XRP; the pool's XRP is what buyers paid in, and what the keeper deposited from the faucet for depth.

## Its VM

`vm/` holds what runs the keeper on GCP, the way `scripts/sim-campaign/` runs the campaign ([Testing](testing.md#the-campaign)):

- `provision.sh` prints the gcloud commands that make it, and runs them with `--apply`: a service account that can read only the two secrets that hold the seeds, a network of its own with no firewall rule, so nothing reaches the VM, and a small VM in a region where one is free.
- `startup.sh`, the VM's boot script, installs Nix, clones the repository and installs the keeper's timer and service, which take effect at each boot.
- The service runs the keeper for its life; the timer starts it again a few seconds after it ended, for whatever reason. Each start checks out the newest `main`, so a change there reaches the keeper within the hour. `vm/run.sh` reads the seeds from Secret Manager into the process's environment only, installs the keeper's dependency from the lockfile, and runs it with Node of the flake's nixpkgs.
- The keeper's lines go to the journal and the serial console, which can be read without SSH.

## Testing

The pure parts are separate modules, each with `node --test` unit tests that never touch the network: the price from a ticker answer, the pool's math with the fee, the ledger's answers parsed, the transactions each step submits, the settings, the plan, and when a following keeper runs and what each window spends, on a clock the tests move. The plan's tests run whole runs against a model of the ledger that checks what the ledger checks (enough XRP, DSUSD and LP tokens for each step), from a fresh testnet, after traders moved the pool and the market moved, after a reset, and over a long seeded walk of both; each run must end at the target, within its budget, and without a step that pays the keeper XRP. The keeper itself is checked by hand on the testnet with fresh seeds, which also stands for a reset, and a trade against the pool it makes.

## Decisions

- [dsusd](../decisions/dsusd.md): DSUSD beside RLUSD, bought only with XRP through the pool, one exchange's price, and repricing with DSUSD alone.
- [keeper-follows-the-pool](../decisions/keeper-follows-the-pool.md): repricing after each trade, with a budget per window of time.
- [open-rate-sources](../decisions/open-rate-sources.md): mainnet's XRP/RLUSD pool as the target, in place of the exchange's ticker (not built).
- [issued-assets](../decisions/issued-assets.md): a dollar token is booked as `USD`, at par.
