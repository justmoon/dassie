//! Tokens on trust lines in the fake ledger (spec/areas/xrpl.md#the-fake-ledger):
//! lines as `RippleState` entries with their owner reserve, `TrustSet`,
//! token Payments that ripple through the issuer with the ledger's
//! failures, and the issuer's own transactions: minting, freezes, a
//! transfer rate and clawback.
//!
//! Arithmetic is exact. Where the ledger would round a balance to sixteen
//! digits, the fake refuses the transaction (`tecPRECISION_LOSS`), so no
//! test depends on how the ledger rounds.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::cmp::Ordering;

use sha2::{Digest, Sha512};

use super::{
    Account, DSUSD_CURRENCY, DSUSD_ISSUER, FEE_BASE, FakeLedger, Outcome, RLUSD_CURRENCY,
    RLUSD_ISSUER, Touched, Tx,
};
use crate::api::{TRANSFER_RATE_PAR, TrustLine, account_flags, line_flags};
use crate::codec::{
    Amount, Currency, Payment, TF_CLEAR_DEEP_FREEZE, TF_CLEAR_FREEZE, TF_CLEAR_NO_RIPPLE,
    TF_SET_DEEP_FREEZE, TF_SET_FREEZE, TF_SET_NO_RIPPLE, TokenAmount, TokenValue, TrustSet,
};
use crate::keys::AccountId;

/// A line's key: its low account, its high account, its currency.
pub(super) type LineKey = (AccountId, AccountId, Currency);

/// AccountSet's flag number (`asf`) for a global freeze.
pub const ASF_GLOBAL_FREEZE: u32 = 7;
/// The highest transfer rate, in billionths: a fee of 100 %.
pub const MAX_TRANSFER_RATE: u32 = 2_000_000_000;
/// TrustSet's flag that authorises a holder, for an issuer that requires
/// it; the fake's issuers do not.
const TF_SETF_AUTH: u32 = 0x0001_0000;
/// The TrustSet flags the fake knows, with `tfFullyCanonicalSig`.
const TRUST_SET_FLAGS: u32 = TF_SET_NO_RIPPLE
    | TF_CLEAR_NO_RIPPLE
    | TF_SET_FREEZE
    | TF_CLEAR_FREEZE
    | TF_SET_DEEP_FREEZE
    | TF_CLEAR_DEEP_FREEZE
    | 0x8000_0000;

/// A token: a currency of an issuer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Token {
    /// Who issues it.
    pub issuer: AccountId,
    /// Its currency code.
    pub currency: Currency,
}

impl Token {
    /// RLUSD of its testnet issuer, which the fake ledger issues.
    pub const RLUSD: Self = Self {
        issuer: RLUSD_ISSUER,
        currency: RLUSD_CURRENCY,
    };

    /// DSUSD of its testnet issuer, which the fake ledger issues too.
    pub const DSUSD: Self = Self {
        issuer: DSUSD_ISSUER,
        currency: DSUSD_CURRENCY,
    };

    /// The token of the fake's own issuers whose currency code is `code`,
    /// such as `"RLUSD"`.
    #[must_use]
    pub fn by_code(code: &str) -> Option<Self> {
        [Self::RLUSD, Self::DSUSD]
            .into_iter()
            .find(|token| token.currency.code() == code)
    }

    /// `value` of it.
    #[must_use]
    pub const fn amount(self, value: TokenValue) -> TokenAmount {
        TokenAmount {
            value,
            currency: self.currency,
            issuer: self.issuer,
        }
    }
}

/// An exact sum of token values, however far apart their exponents: what
/// a ledger's sixteen digits cannot always hold. It only says whether it
/// is below, at or above zero.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExactSum {
    /// Mantissas summed, by exponent.
    terms: BTreeMap<i32, i128>,
}

impl ExactSum {
    /// Adds `value`.
    pub fn add(&mut self, value: TokenValue) {
        if value.is_zero() {
            return;
        }
        let mantissa = i128::from(value.mantissa());
        let term = self.terms.entry(value.exponent()).or_insert(0);
        let next = term.checked_add(if value.is_negative() {
            -mantissa
        } else {
            mantissa
        });
        assert!(next.is_some(), "an exact sum fits an i128 per exponent");
        *term = next.unwrap_or_default();
    }

    /// Takes `value` away.
    pub fn sub(&mut self, value: TokenValue) {
        self.add(value.negate());
    }

    /// Takes `other` away.
    pub fn sub_sum(&mut self, other: &Self) {
        for (exponent, mantissas) in &other.terms {
            let term = self.terms.entry(*exponent).or_insert(0);
            let next = term.checked_sub(*mantissas);
            assert!(next.is_some(), "an exact sum fits an i128 per exponent");
            *term = next.unwrap_or_default();
        }
    }

    /// Whether it is below, at or above zero.
    #[must_use]
    pub fn signum(&self) -> Ordering {
        let (Some((&first, _)), Some((&last, _))) =
            (self.terms.first_key_value(), self.terms.last_key_value())
        else {
            return Ordering::Equal;
        };
        // Carried digit by digit from the lowest exponent: each digit in
        // 0..=9, and what is carried past the last settles at 0 or -1,
        // which is below every sum of digits beneath it.
        let (mut carry, mut digits, mut exponent) = (0_i128, false, first);
        loop {
            let term = self.terms.get(&exponent).copied().unwrap_or(0);
            let total = carry.checked_add(term);
            assert!(total.is_some(), "carries stay below the terms");
            let total = total.unwrap_or_default();
            digits |= total.rem_euclid(10) != 0;
            carry = total.div_euclid(10);
            if exponent >= last && (carry == 0 || carry == -1) {
                break;
            }
            exponent = exponent.saturating_add(1);
        }
        if carry < 0 {
            Ordering::Less
        } else if digits {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    }
}

/// An issuer's `AccountSet`: a global freeze set or cleared, or a
/// transfer rate.
#[derive(Clone, Debug)]
pub(super) struct IssuerSet {
    pub account: AccountId,
    pub sequence: u32,
    pub last_ledger_sequence: u32,
    pub set_flag: Option<u32>,
    pub clear_flag: Option<u32>,
    pub transfer_rate: Option<u32>,
}

/// An issuer's `Clawback`: as the ledger writes it, the amount names the
/// holder as its issuer.
#[derive(Clone, Debug)]
pub(super) struct Clawback {
    pub account: AccountId,
    pub amount: TokenAmount,
    pub sequence: u32,
    pub last_ledger_sequence: u32,
}

/// A token's move between two accounts, checked and not yet made.
pub(super) struct TokenMove {
    from: AccountId,
    to: AccountId,
    amount: TokenAmount,
    /// The lines it goes over, after it.
    after: Vec<TrustLine>,
    /// Whether it goes through the issuer, which neither account is.
    through: bool,
}

/// The key of the line between `a` and `b` in `currency`.
pub(super) fn line_key(a: AccountId, b: AccountId, currency: Currency) -> LineKey {
    if a < b {
        (a, b, currency)
    } else {
        (b, a, currency)
    }
}

/// A line's ledger index, as the ledger keys it: SHA-512Half of its space
/// (`r`), its accounts and its currency.
pub(super) fn line_index((low, high, currency): &LineKey) -> [u8; 32] {
    let mut hasher = Sha512::new();
    hasher.update([0x00, b'r']);
    hasher.update(low.0);
    hasher.update(high.0);
    hasher.update(currency.0);
    let digest = hasher.finalize();
    let mut out = [0; 32];
    out.copy_from_slice(digest.get(..32).unwrap_or(&[0; 32]));
    out
}

/// What a TrustSet alone may be refused for.
pub(super) fn preflight_trust_set(trust_set: &TrustSet) -> Result<(), &'static str> {
    let flags = trust_set.flags;
    if flags & TF_SETF_AUTH != 0 {
        return Err("tefNO_AUTH_REQUIRED");
    }
    if flags & !TRUST_SET_FLAGS != 0 {
        return Err("temINVALID_FLAG");
    }
    let both = |set: u32, clear: u32| flags & set != 0 && flags & clear != 0;
    if both(TF_SET_NO_RIPPLE, TF_CLEAR_NO_RIPPLE)
        || both(TF_SET_FREEZE, TF_CLEAR_FREEZE)
        || both(TF_SET_DEEP_FREEZE, TF_CLEAR_DEEP_FREEZE)
    {
        return Err("temINVALID_FLAG");
    }
    if trust_set.limit.issuer == trust_set.account {
        return Err("temDST_IS_SRC");
    }
    Ok(())
}

/// What `from` can pay over `line`: what it holds, and what the other
/// side lets it owe.
fn room(line: &TrustLine, from: AccountId) -> ExactSum {
    let mut room = ExactSum::default();
    if let Some(view) = line.view(from) {
        room.add(view.balance);
        room.add(view.counterparty_limit);
    }
    room
}

/// `line` after `from` paid `value` over it, if the ledger can hold the
/// balance exactly.
fn moved(line: &TrustLine, from: AccountId, value: TokenValue) -> Option<TrustLine> {
    let balance = if from == line.low {
        line.balance.checked_sub(value)?
    } else {
        line.balance.checked_add(value)?
    };
    Some(TrustLine { balance, ..*line })
}

impl FakeLedger {
    /// The line between `a` and `b` in `currency`, if there is one.
    #[must_use]
    pub fn line(&self, a: AccountId, b: AccountId, currency: Currency) -> Option<TrustLine> {
        self.lines.get(&line_key(a, b, currency)).copied()
    }

    /// What `holder` holds of `token`, if it has a line to its issuer.
    #[must_use]
    pub fn token_balance(&self, holder: AccountId, token: Token) -> Option<TokenValue> {
        self.line(holder, token.issuer, token.currency)?
            .view(holder)
            .map(|view| view.balance)
    }

    /// Whether, for every token of every issuer, its holders hold exactly
    /// what the issuer minted less what came back to it, was clawed back
    /// or wiped by a reset: payments between holders make or lose nothing.
    #[must_use]
    pub fn tokens_conserved(&self) -> bool {
        let mut held: BTreeMap<Token, ExactSum> = BTreeMap::new();
        for line in self.lines.values() {
            for issuer in [line.low, line.high] {
                if !self.issuers.contains(&issuer) {
                    continue;
                }
                let holder = if issuer == line.low {
                    line.high
                } else {
                    line.low
                };
                if let Some(view) = line.view(holder) {
                    let token = Token {
                        issuer,
                        currency: line.currency,
                    };
                    held.entry(token).or_default().add(view.balance);
                }
            }
        }
        let tokens: Vec<Token> = held.keys().chain(self.supply.keys()).copied().collect();
        tokens.into_iter().all(|token| {
            let mut difference = held.get(&token).cloned().unwrap_or_default();
            if let Some(supply) = self.supply.get(&token) {
                difference.sub_sum(supply);
            }
            difference.signum() == Ordering::Equal
        })
    }

    /// The issuer pays `amount` of its token to `to` in the next ledger,
    /// as any Payment: it fails without a line, or above its limit.
    ///
    /// # Errors
    ///
    /// The amount's issuer is none of the fake's (`terNO_ACCOUNT`), the
    /// amount is not above zero (`temBAD_AMOUNT`), or `to` is the issuer
    /// (`temREDUNDANT`).
    pub fn issue(&mut self, to: AccountId, amount: TokenAmount) -> Result<(), &'static str> {
        let (sequence, last_ledger_sequence) = self.own_slot(amount.issuer)?;
        if amount.value.is_zero() || amount.value.is_negative() || !amount.is_valid() {
            return Err("temBAD_AMOUNT");
        }
        if to == amount.issuer {
            return Err("temREDUNDANT");
        }
        self.queue_own(Tx::Payment(Payment {
            account: amount.issuer,
            destination: to,
            amount: Amount::Token(amount),
            send_max: None,
            fee: FEE_BASE,
            sequence,
            last_ledger_sequence,
            flags: 0,
            network_id: None,
            memos: Vec::new(),
            signing_pub_key: [0; 33],
            signature: None,
        }));
        Ok(())
    }

    /// The issuer freezes `holder`'s line, or lifts its freeze and any
    /// deep freeze, in the next ledger.
    ///
    /// # Errors
    ///
    /// As [`issue`](Self::issue): the issuer is none of the fake's, or is
    /// the holder.
    pub fn freeze(
        &mut self,
        holder: AccountId,
        token: Token,
        on: bool,
    ) -> Result<(), &'static str> {
        let flags = if on {
            TF_SET_FREEZE
        } else {
            TF_CLEAR_FREEZE | TF_CLEAR_DEEP_FREEZE
        };
        self.issuer_trust_set(holder, token, flags)
    }

    /// The issuer deep-freezes `holder`'s line, freezing it too, or lifts
    /// the deep freeze and keeps the freeze, in the next ledger.
    ///
    /// # Errors
    ///
    /// As [`freeze`](Self::freeze).
    pub fn deep_freeze(
        &mut self,
        holder: AccountId,
        token: Token,
        on: bool,
    ) -> Result<(), &'static str> {
        let flags = if on {
            TF_SET_FREEZE | TF_SET_DEEP_FREEZE
        } else {
            TF_CLEAR_DEEP_FREEZE
        };
        self.issuer_trust_set(holder, token, flags)
    }

    fn issuer_trust_set(
        &mut self,
        holder: AccountId,
        token: Token,
        flags: u32,
    ) -> Result<(), &'static str> {
        let (sequence, last_ledger_sequence) = self.own_slot(token.issuer)?;
        if holder == token.issuer {
            return Err("temDST_IS_SRC");
        }
        // The issuer keeps the limit it has on the line: none.
        let limit = self
            .line(holder, token.issuer, token.currency)
            .and_then(|line| line.view(token.issuer))
            .map_or(TokenValue::ZERO, |view| view.limit);
        self.queue_own(Tx::TrustSet(TrustSet {
            account: token.issuer,
            limit: TokenAmount {
                value: limit,
                currency: token.currency,
                issuer: holder,
            },
            fee: FEE_BASE,
            sequence,
            last_ledger_sequence,
            flags,
            network_id: None,
            memos: Vec::new(),
            signing_pub_key: [0; 33],
            signature: None,
        }));
        Ok(())
    }

    /// The issuer freezes every line of its tokens, or lifts that, in the
    /// next ledger (`AccountSet`).
    ///
    /// # Errors
    ///
    /// The issuer is none of the fake's.
    pub fn global_freeze(&mut self, issuer: AccountId, on: bool) -> Result<(), &'static str> {
        let (sequence, last_ledger_sequence) = self.own_slot(issuer)?;
        let flag = Some(ASF_GLOBAL_FREEZE);
        self.queue_own(Tx::AccountSet(IssuerSet {
            account: issuer,
            sequence,
            last_ledger_sequence,
            set_flag: flag.filter(|_| on),
            clear_flag: flag.filter(|_| !on),
            transfer_rate: None,
        }));
        Ok(())
    }

    /// The issuer sets its transfer rate, in billionths, in the next
    /// ledger: 0 or [`TRANSFER_RATE_PAR`] charge nothing.
    ///
    /// # Errors
    ///
    /// The issuer is none of the fake's, or the rate is beyond the
    /// ledger's (`temBAD_TRANSFER_RATE`).
    pub fn set_transfer_rate(&mut self, issuer: AccountId, rate: u32) -> Result<(), &'static str> {
        let (sequence, last_ledger_sequence) = self.own_slot(issuer)?;
        if rate != 0 && !(TRANSFER_RATE_PAR..=MAX_TRANSFER_RATE).contains(&rate) {
            return Err("temBAD_TRANSFER_RATE");
        }
        self.queue_own(Tx::AccountSet(IssuerSet {
            account: issuer,
            sequence,
            last_ledger_sequence,
            set_flag: None,
            clear_flag: None,
            transfer_rate: Some(rate),
        }));
        Ok(())
    }

    /// The issuer of `amount` claws it back from `holder` in the next
    /// ledger (`Clawback`), or all `holder` holds if that is less.
    ///
    /// # Errors
    ///
    /// The issuer is none of the fake's, or the amount is not above zero
    /// or `holder` is the issuer (`temBAD_AMOUNT`).
    pub fn claw_back(
        &mut self,
        holder: AccountId,
        amount: TokenAmount,
    ) -> Result<(), &'static str> {
        let (sequence, last_ledger_sequence) = self.own_slot(amount.issuer)?;
        if amount.value.is_zero() || amount.value.is_negative() || holder == amount.issuer {
            return Err("temBAD_AMOUNT");
        }
        self.queue_own(Tx::Clawback(Clawback {
            account: amount.issuer,
            amount: TokenAmount {
                issuer: holder,
                ..amount
            },
            sequence,
            last_ledger_sequence,
        }));
        Ok(())
    }

    /// A Payment of a token, after its fee: over the line between `from`
    /// and `to` if either issues it, else through its issuer.
    pub(super) fn pay_token(
        &mut self,
        touched: &mut Touched,
        from: AccountId,
        to: AccountId,
        amount: TokenAmount,
    ) -> Outcome {
        if !self.accounts.contains_key(&to) {
            return Outcome::code("tecNO_DST");
        }
        match self.plan_token(from, to, amount) {
            Ok(planned) => {
                self.move_token(touched, planned);
                Outcome {
                    code: "tesSUCCESS",
                    delivered: Some(Amount::Token(amount)),
                }
            }
            Err(code) => Outcome::code(code),
        }
    }

    /// How `amount` would go from `from` to `to`, over the line between
    /// them if either issues it, else through its issuer, checked as the
    /// ledger checks it; or the code it fails with. Nothing changes.
    pub(super) fn plan_token(
        &self,
        from: AccountId,
        to: AccountId,
        amount: TokenAmount,
    ) -> Result<TokenMove, &'static str> {
        let issuer = amount.issuer;
        let hops: Vec<(AccountId, AccountId)> = if issuer == from || issuer == to {
            alloc::vec![(from, to)]
        } else {
            alloc::vec![(from, issuer), (issuer, to)]
        };
        let mut lines = Vec::with_capacity(hops.len());
        for (payer, payee) in hops {
            match self.line(payer, payee, amount.currency) {
                Some(line) => lines.push((payer, line)),
                None => return Err("tecPATH_DRY"),
            }
        }
        let through = lines.len() == 2;
        if through && self.closed_through(issuer, &lines) {
            return Err("tecPATH_DRY");
        }
        let (mut dry, mut short) = (false, false);
        for (payer, line) in &lines {
            let mut room = room(line, *payer);
            dry |= room.signum() != Ordering::Greater;
            room.sub(amount.value);
            short |= room.signum() == Ordering::Less;
        }
        // The fake takes no issuer's fee on top, `SendMax` or not.
        let fee = through && self.charges_transfer_fee(issuer);
        if dry {
            return Err("tecPATH_DRY");
        }
        if short || fee {
            return Err("tecPATH_PARTIAL");
        }
        let after = lines
            .iter()
            .map(|(payer, line)| moved(line, *payer, amount.value))
            .collect::<Option<Vec<_>>>()
            .ok_or("tecPRECISION_LOSS")?;
        Ok(TokenMove {
            from,
            to,
            amount,
            after,
            through,
        })
    }

    /// Makes a move [planned](Self::plan_token) on the ledger as it still
    /// is.
    pub(super) fn move_token(&mut self, touched: &mut Touched, planned: TokenMove) {
        for line in planned.after {
            self.store_line(touched, line);
        }
        if !planned.through {
            self.count_supply(planned.from, planned.to, planned.amount);
        }
    }

    /// Whether the issuer lets nothing through from the first line to the
    /// second: a global freeze, the payer's line frozen, the payee's
    /// deep-frozen, or rippling disabled on both of its sides.
    fn closed_through(&self, issuer: AccountId, lines: &[(AccountId, TrustLine)]) -> bool {
        let global = self
            .accounts
            .get(&issuer)
            .is_some_and(|account| account.flags & account_flags::GLOBAL_FREEZE != 0);
        let side = |index: usize| {
            lines
                .get(index)
                .and_then(|(_, line)| line.view(issuer))
                .map(|view| view.ours)
                .unwrap_or_default()
        };
        let (payer, payee) = (side(0), side(1));
        global || payer.freeze || payee.deep_freeze || (payer.no_ripple && payee.no_ripple)
    }

    fn charges_transfer_fee(&self, issuer: AccountId) -> bool {
        self.accounts
            .get(&issuer)
            .is_some_and(|account| !matches!(account.transfer_rate, 0 | TRANSFER_RATE_PAR))
    }

    /// What one of the fake's issuers minted (paid a holder) or took back
    /// (was paid), for [`tokens_conserved`](Self::tokens_conserved).
    fn count_supply(&mut self, from: AccountId, to: AccountId, amount: TokenAmount) {
        let token = |issuer| Token {
            issuer,
            currency: amount.currency,
        };
        if self.issuers.contains(&from) {
            self.supply
                .entry(token(from))
                .or_default()
                .add(amount.value);
        } else if self.issuers.contains(&to) {
            self.supply.entry(token(to)).or_default().sub(amount.value);
        }
    }

    /// Whether `side`'s half of `line` is not as the ledger leaves it by
    /// default, so that it holds an owner reserve: a limit, NoRipple other
    /// than its account's default, a freeze, or a balance it holds.
    fn needs_reserve(&self, line: &TrustLine, side: AccountId) -> bool {
        let Some(view) = line.view(side) else {
            return false;
        };
        let default_no_ripple = self
            .accounts
            .get(&side)
            .is_none_or(|account| account.flags & account_flags::DEFAULT_RIPPLE == 0);
        !view.limit.is_zero()
            || view.ours.no_ripple != default_no_ripple
            || view.ours.freeze
            || view.ours.deep_freeze
            || view.balance > TokenValue::ZERO
    }

    /// Writes a line: each side's reserve follows its state, with its
    /// account's owner count, and a line in its default state on both
    /// sides is deleted.
    pub(super) fn store_line(&mut self, touched: &mut Touched, line: TrustLine) {
        let key = (line.low, line.high, line.currency);
        let before = self.lines.get(&key).copied();
        let mut line = line;
        for (side, bit) in [
            (line.low, line_flags::LOW_RESERVE),
            (line.high, line_flags::HIGH_RESERVE),
        ] {
            let needs = self.needs_reserve(&line, side);
            if needs == (line.flags & bit != 0) {
                continue;
            }
            line.flags ^= bit;
            if let Some(mut account) = self.accounts.get(&side).copied() {
                account.owner_count = if needs {
                    account.owner_count.saturating_add(1)
                } else {
                    account.owner_count.saturating_sub(1)
                };
                self.put_account(touched, side, account);
            }
        }
        touched.line(key, before, line);
        let reserves = line_flags::LOW_RESERVE | line_flags::HIGH_RESERVE;
        if line.flags & reserves == 0 && line.balance.is_zero() {
            self.lines.remove(&key);
        } else {
            self.lines.insert(key, line);
        }
    }

    /// A TrustSet, after its fee: sets the sender's limit and flags on its
    /// line to the other account, making the line if needed. `prior` is
    /// the sender's XRP before the fee, which the reserve is checked
    /// against.
    pub(super) fn trust_set(
        &mut self,
        touched: &mut Touched,
        trust_set: &TrustSet,
        prior: u64,
    ) -> &'static str {
        let (account, currency) = (trust_set.account, trust_set.limit.currency);
        let peer = trust_set.limit.issuer;
        let (Some(sender), Some(other)) = (
            self.accounts.get(&account).copied(),
            self.accounts.get(&peer).copied(),
        ) else {
            return "tecNO_DST";
        };
        let freezing = trust_set.flags & (TF_SET_FREEZE | TF_SET_DEEP_FREEZE) != 0;
        if freezing && sender.flags & account_flags::NO_FREEZE != 0 {
            return "tecNO_PERMISSION";
        }
        let existing = self.line(account, peer, currency);
        if existing.is_none() && trust_set.limit.value.is_zero() {
            return "tecNO_LINE_REDUNDANT";
        }
        let line = with_side_set(
            existing.unwrap_or_else(|| new_line(account, peer, currency, &other)),
            account,
            trust_set,
        );
        let Some(view) = line.view(account) else {
            return "tefINTERNAL";
        };
        if view.ours.deep_freeze && !view.ours.freeze {
            return "tecNO_PERMISSION";
        }
        if trust_set.flags & TF_SET_NO_RIPPLE != 0 && view.balance.is_negative() {
            return "tecNO_PERMISSION";
        }
        let bit = if account == line.low {
            line_flags::LOW_RESERVE
        } else {
            line_flags::HIGH_RESERVE
        };
        let had = existing.is_some_and(|line| line.flags & bit != 0);
        if self.needs_reserve(&line, account) && !had && prior < self.reserve_to_own(&sender) {
            return if existing.is_none() {
                "tecNO_LINE_INSUF_RESERVE"
            } else {
                "tecINSUF_RESERVE_LINE"
            };
        }
        self.store_line(touched, line);
        "tesSUCCESS"
    }

    /// What an account must hold to own one more line: nothing while it
    /// owns fewer than two objects, as the ledger lets an account make its
    /// first two lines on the base reserve alone.
    fn reserve_to_own(&self, account: &Account) -> u64 {
        if account.owner_count < 2 {
            return 0;
        }
        self.reserve.0.saturating_add(
            self.reserve
                .1
                .saturating_mul(u64::from(account.owner_count).saturating_add(1)),
        )
    }

    /// An issuer's AccountSet, after its fee.
    pub(super) fn issuer_set(&mut self, touched: &mut Touched, set: &IssuerSet) -> &'static str {
        let Some(mut account) = self.accounts.get(&set.account).copied() else {
            return "tefINTERNAL";
        };
        if set.set_flag == Some(ASF_GLOBAL_FREEZE) {
            account.flags |= account_flags::GLOBAL_FREEZE;
        }
        if set.clear_flag == Some(ASF_GLOBAL_FREEZE) {
            account.flags &= !account_flags::GLOBAL_FREEZE;
        }
        if let Some(rate) = set.transfer_rate {
            account.transfer_rate = if rate == TRANSFER_RATE_PAR { 0 } else { rate };
        }
        self.put_account(touched, set.account, account);
        "tesSUCCESS"
    }

    /// An issuer's Clawback, after its fee: lowers the holder's balance by
    /// the amount, or to zero, with no Payment.
    pub(super) fn claw(&mut self, touched: &mut Touched, clawback: &Clawback) -> &'static str {
        let (issuer, holder) = (clawback.account, clawback.amount.issuer);
        let flags = self
            .accounts
            .get(&issuer)
            .map_or(0, |account| account.flags);
        if flags & account_flags::ALLOW_TRUST_LINE_CLAWBACK == 0
            || flags & account_flags::NO_FREEZE != 0
        {
            return "tecNO_PERMISSION";
        }
        let Some(line) = self.line(holder, issuer, clawback.amount.currency) else {
            return "tecNO_LINE";
        };
        let held = line
            .view(holder)
            .map_or(TokenValue::ZERO, |view| view.balance);
        if held <= TokenValue::ZERO {
            return "tecNO_LINE";
        }
        let clawed = clawback.amount.value.min(held);
        let Some(after) = moved(&line, holder, clawed) else {
            return "tecPRECISION_LOSS";
        };
        self.store_line(touched, after);
        let token = Token {
            issuer,
            currency: line.currency,
        };
        self.supply.entry(token).or_default().sub(clawed);
        "tesSUCCESS"
    }
}

/// A line `account` is about to make to `peer`: nothing on it, and
/// rippling disabled on the peer's side unless the peer ripples by
/// default.
pub(super) fn new_line(
    account: AccountId,
    peer: AccountId,
    currency: Currency,
    other: &Account,
) -> TrustLine {
    let (low, high) = if account < peer {
        (account, peer)
    } else {
        (peer, account)
    };
    let mut flags = 0;
    if other.flags & account_flags::DEFAULT_RIPPLE == 0 {
        flags |= if peer == low {
            line_flags::LOW_NO_RIPPLE
        } else {
            line_flags::HIGH_NO_RIPPLE
        };
    }
    TrustLine {
        low,
        high,
        currency,
        balance: TokenValue::ZERO,
        low_limit: TokenValue::ZERO,
        high_limit: TokenValue::ZERO,
        flags,
    }
}

/// `line` with `account`'s limit and flags as the TrustSet sets them.
fn with_side_set(line: TrustLine, account: AccountId, trust_set: &TrustSet) -> TrustLine {
    let low = account == line.low;
    let pick = |low_bit: u32, high_bit: u32| if low { low_bit } else { high_bit };
    let mut line = line;
    if low {
        line.low_limit = trust_set.limit.value;
    } else {
        line.high_limit = trust_set.limit.value;
    }
    for (set, clear, bit) in [
        (
            TF_SET_NO_RIPPLE,
            TF_CLEAR_NO_RIPPLE,
            pick(line_flags::LOW_NO_RIPPLE, line_flags::HIGH_NO_RIPPLE),
        ),
        (
            TF_SET_FREEZE,
            TF_CLEAR_FREEZE,
            pick(line_flags::LOW_FREEZE, line_flags::HIGH_FREEZE),
        ),
        (
            TF_SET_DEEP_FREEZE,
            TF_CLEAR_DEEP_FREEZE,
            pick(line_flags::LOW_DEEP_FREEZE, line_flags::HIGH_DEEP_FREEZE),
        ),
    ] {
        if trust_set.flags & set != 0 {
            line.flags |= bit;
        }
        if trust_set.flags & clear != 0 {
            line.flags &= !bit;
        }
    }
    line
}
