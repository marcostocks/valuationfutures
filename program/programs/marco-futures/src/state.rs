use anchor_lang::prelude::*;

use crate::errors::FuturesError;

// ═══════════════════════════════════════════════════════════════
// MARCO FUTURES STATE
//
// One `Market` == one valuation future on a single private company
// (e.g. Moonshot AI). It is a DATED future: traders take leveraged
// long/short exposure against a virtual AMM (vAMM), and at the real
// listing / acquisition event the admin posts a settlement price and
// every position settles at that fixed number.
//
// Unlike the vaults, this program IS the counterparty. Trader USDC
// margin lives in a program-owned `collateral_vault`; profits are paid
// out of it, and a per-market `insurance_vault` backstops the residual
// skew the vAMM leaves open. Two invariants make that safe:
//
//   1. SOLVENCY BY CONSTRUCTION. No instruction ever transfers more
//      USDC out of a vault than the vault actually holds. Payouts are
//      clamped to the available balance; any gap is an explicit,
//      emitted shortfall, never an over-draft.
//   2. ISOLATION. Margin, open interest, reserves and insurance are all
//      per-market. One market cannot drain another.
//
// PRICE / VALUE UNITS
// - USDC is 6 decimals. Every `margin`, `notional`, `open_notional`,
//   fee and payout below is in native USDC units (6dp), as u64.
// - `PRICE_PRECISION` (1e6) scales a price. A "price" here is the vAMM
//   ratio quote_reserve / base_reserve, and doubles as the company's
//   valuation index — e.g. a $12.5B anchor is price 12_500_000.
// - vAMM reserves are u128. `base_size` and PnL are i128 (signed:
//   +long, -short). All ratio math uses u128/i128 intermediates and
//   only narrows to u64 at the edges, with checked bounds.
// ═══════════════════════════════════════════════════════════════

/// Fixed-point scale for a price (the vAMM quote/base ratio). 1.0 == 1e6.
pub const PRICE_PRECISION: u128 = 1_000_000;

/// Basis-point denominator. 10_000 bps == 100%.
pub const BPS_DENOMINATOR: u128 = 10_000;

/// The lifecycle of a single valuation-futures market.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarketStatus {
    /// Trading is open: positions can be opened, closed and liquidated.
    Active,
    /// Emergency halt. No new exposure (no opens); closing, liquidation
    /// and — once settled — settlement still work. Reversible by admin.
    Halted,
    /// The underlying has listed/been acquired and `settlement_price` is
    /// fixed. No trading; holders settle each position at that price.
    Settled,
}

impl Default for MarketStatus {
    fn default() -> Self {
        MarketStatus::Active
    }
}

/// The side a position is opened on.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Long,
    Short,
}

/// The outcome of a vAMM swap: the base delta the trader receives
/// (signed: +long, -short) and the reserves after the swap. The caller
/// commits the new reserves; the swap function itself is pure.
pub struct SwapResult {
    /// Base acquired by the trader. Positive for a long, negative for a short.
    pub base_delta: i128,
    pub new_base_reserve: u128,
    pub new_quote_reserve: u128,
}

// ─────────────────────────── FuturesConfig ───────────────────────────

/// Protocol-global config. One per deployment. Holds the authorities and
/// the collateral mint; per-market economics (fees, leverage, margin) live
/// on each `Market`, seeded from `create_market` params.
#[account]
pub struct FuturesConfig {
    /// PDA bump. Seeds: [b"config"].
    pub bump: u8,

    /// Controlling authority — expected to be a Squads multisig PDA. Can
    /// create markets, halt, settle, and rotate the operator.
    pub admin: Pubkey,

    /// Operator wallet — the off-chain keeper. May settle and run
    /// liquidations; cannot move funds to itself (can equal admin).
    pub operator: Pubkey,

    /// Treasury wallet — reserved for future fee sweeps. In v1 all trading
    /// fees seed the per-market insurance fund instead.
    pub treasury: Pubkey,

    /// The one collateral mint every market accepts (USDC, 6 decimals).
    pub usdc_mint: Pubkey,

    /// Emergency kill-switch. When true, no market accepts new opens.
    pub paused: bool,

    /// Number of markets created (monotonic; informational).
    pub market_count: u64,

    /// Reserved for forward-compatible upgrades.
    pub _reserved: [u8; 128],
}

impl FuturesConfig {
    /// 8 (disc) + 1 (bump) + 32*4 (pubkeys) + 1 (paused) + 8 (market_count)
    /// + 128 (reserved).
    pub const MAX_SIZE: usize = 8 + 1 + (32 * 4) + 1 + 8 + 128;
}

// ──────────────────────────── Market ────────────────────────────

#[account]
pub struct Market {
    /// PDA bump. Seeds: [b"market", config, market_id].
    pub bump: u8,

    /// Back-reference to the owning `FuturesConfig`.
    pub config: Pubkey,

    /// Human-readable id, e.g. "moon-2026". Max 32 bytes.
    pub market_id: String,

    /// Collateral mint (USDC), copied from config for local validation.
    pub usdc_mint: Pubkey,

    /// Program-owned USDC account holding ALL trader margin for this market.
    /// Owned by the market PDA; only this program can move funds out.
    pub collateral_vault: Pubkey,

    /// Program-owned USDC account backstopping this market. Seeded by
    /// `seed_insurance` and fed by trading fees; drawn on to cover payouts
    /// the collateral vault alone cannot.
    pub insurance_vault: Pubkey,

    /// Lifecycle status.
    pub status: MarketStatus,

    // ── vAMM (constant product) ──
    /// Virtual base reserve (signed exposure denominated in base units,
    /// same 1e6 scale as a price of 1.0). Falls as the pool goes net-long.
    pub base_reserve: u128,
    /// Virtual quote reserve (USDC-equivalent depth). Rises as the pool
    /// takes in long notional. Mark price == quote_reserve / base_reserve.
    pub quote_reserve: u128,
    /// Reserves at creation, retained so net trader exposure and the
    /// distance travelled from the anchor can be read back.
    pub initial_base_reserve: u128,
    pub initial_quote_reserve: u128,

    /// The seed valuation index (1e6). Mark price equals this at creation.
    pub anchor_price: u64,

    // ── risk parameters (fixed at creation for v1) ──
    /// Maximum leverage, e.g. 3 == 3x. Initial margin ratio == 1 / max_leverage.
    pub max_leverage: u8,
    /// Maintenance margin ratio in bps, e.g. 625 == 6.25%. Below this a
    /// position is liquidatable.
    pub maintenance_margin_bps: u16,
    /// Taker fee in bps charged on notional at open and close, e.g. 30 == 0.30%.
    /// In v1 the whole fee is routed to the insurance vault.
    pub taker_fee_bps: u16,
    /// Liquidation penalty in bps of the liquidated notional, split between
    /// the liquidator (keeper reward) and the insurance vault.
    pub liquidation_fee_bps: u16,
    /// Cap on the open notional per side (long and short each). 0 == no cap.
    pub max_oi_notional: u64,

    // ── running totals ──
    /// Sum of `open_notional` across open long positions.
    pub long_open_notional: u64,
    /// Sum of `open_notional` across open short positions.
    pub short_open_notional: u64,
    /// Sum of `margin` across all open positions (accounting mirror; the
    /// collateral vault balance is the source of truth for solvency).
    pub total_collateral: u64,
    /// Open positions in this market (informational).
    pub position_count: u64,

    // ── settlement ──
    /// Final settlement price (1e6), 0 until the market is Settled.
    pub settlement_price: u64,
    /// Informational expected-listing timestamp / dated-expiry fallback.
    pub expiry_ts: i64,
    /// When the market settled (0 until then).
    pub settled_at: i64,
    /// When the market was created.
    pub created_at: i64,

    /// Reserved for forward-compatible upgrades.
    pub _reserved: [u8; 128],
}

impl Market {
    /// 8 (disc) + 1 (bump) + 32 (config) + (4 + 32) (market_id)
    /// + 32 (usdc_mint) + 32 (collateral_vault) + 32 (insurance_vault)
    /// + 1 (status) + 16*4 (reserves) + 8 (anchor_price)
    /// + 1 (max_leverage) + 2 (mmr) + 2 (taker) + 2 (liq) + 8 (max_oi)
    /// + 8 (long_oi) + 8 (short_oi) + 8 (total_collateral) + 8 (position_count)
    /// + 8 (settlement_price) + 8 (expiry) + 8 (settled_at) + 8 (created_at)
    /// + 128 (reserved).
    pub const MAX_SIZE: usize = 8
        + 1
        + 32
        + (4 + 32)
        + 32
        + 32
        + 32
        + 1
        + (16 * 4)
        + 8
        + 1
        + 2
        + 2
        + 2
        + 8
        + 8
        + 8
        + 8
        + 8
        + 8
        + 8
        + 8
        + 8
        + 128;

    /// Longest allowed market id.
    pub const MAX_ID_LEN: usize = 32;
    /// Ceiling on any leverage parameter (guards against a fat-fingered config).
    pub const MAX_LEVERAGE_CAP: u8 = 10;
    /// Ceiling on any bps parameter (100%).
    pub const MAX_BPS: u16 = 10_000;

    pub fn require_status(&self, expected: MarketStatus) -> Result<()> {
        require!(self.status == expected, FuturesError::InvalidMarketStatus);
        Ok(())
    }

    /// The constant-product invariant of the CURRENT reserves. Recomputed
    /// per swap (Uniswap-style) so price is always the live ratio; integer
    /// rounding dust is sub-unit and does not accumulate meaningfully.
    pub fn k(&self) -> Result<u128> {
        self.base_reserve
            .checked_mul(self.quote_reserve)
            .ok_or(error!(FuturesError::MathOverflow))
    }

    /// Mark price (1e6) == quote_reserve * PRICE_PRECISION / base_reserve.
    pub fn mark_price(&self) -> Result<u64> {
        require!(self.base_reserve > 0, FuturesError::MathOverflow);
        let p = self
            .quote_reserve
            .checked_mul(PRICE_PRECISION)
            .ok_or(error!(FuturesError::MathOverflow))?
            .checked_div(self.base_reserve)
            .ok_or(error!(FuturesError::MathOverflow))?;
        u64::try_from(p).map_err(|_| error!(FuturesError::MathOverflow))
    }

    /// Simulate opening `notional` USDC of exposure on `side` against the
    /// current reserves. Pure — returns the base delta and post-swap reserves.
    ///
    /// Long  : quote in, base out  → quote_reserve rises, base_reserve falls.
    /// Short : quote out, base in  → quote_reserve falls, base_reserve rises.
    pub fn quote_open(&self, side: Side, notional: u64) -> Result<SwapResult> {
        require!(notional > 0, FuturesError::ZeroAmount);
        let k = self.k()?;
        let n = notional as u128;
        match side {
            Side::Long => {
                let new_quote = self
                    .quote_reserve
                    .checked_add(n)
                    .ok_or(error!(FuturesError::MathOverflow))?;
                let new_base = k
                    .checked_div(new_quote)
                    .ok_or(error!(FuturesError::MathOverflow))?;
                // Reserves fall as base leaves the pool to the trader.
                let base_out = self
                    .base_reserve
                    .checked_sub(new_base)
                    .ok_or(error!(FuturesError::MathOverflow))?;
                require!(base_out > 0, FuturesError::ZeroAmount);
                Ok(SwapResult {
                    base_delta: i128::try_from(base_out)
                        .map_err(|_| error!(FuturesError::MathOverflow))?,
                    new_base_reserve: new_base,
                    new_quote_reserve: new_quote,
                })
            }
            Side::Short => {
                // Can't withdraw more quote than the pool holds.
                require!(n < self.quote_reserve, FuturesError::InsufficientLiquidity);
                let new_quote = self
                    .quote_reserve
                    .checked_sub(n)
                    .ok_or(error!(FuturesError::MathOverflow))?;
                let new_base = k
                    .checked_div(new_quote)
                    .ok_or(error!(FuturesError::MathOverflow))?;
                // Reserves rise as the trader adds (virtual) base to the pool.
                let base_in = new_base
                    .checked_sub(self.base_reserve)
                    .ok_or(error!(FuturesError::MathOverflow))?;
                require!(base_in > 0, FuturesError::ZeroAmount);
                Ok(SwapResult {
                    base_delta: -(i128::try_from(base_in)
                        .map_err(|_| error!(FuturesError::MathOverflow))?),
                    new_base_reserve: new_base,
                    new_quote_reserve: new_quote,
                })
            }
        }
    }

    /// Simulate closing `base` units of a `side` position against the current
    /// reserves. Returns the quote settled (received for a long, paid for a
    /// short) and the post-swap reserves. Pure.
    ///
    /// This is the exact inverse of `quote_open`: closing a long sells base
    /// back into the pool; closing a short buys base back out.
    pub fn quote_close(&self, side: Side, base: u128) -> Result<(u64, u128, u128)> {
        require!(base > 0, FuturesError::ZeroAmount);
        let k = self.k()?;
        match side {
            Side::Long => {
                // Sell base back: base_reserve rises, quote_reserve falls.
                let new_base = self
                    .base_reserve
                    .checked_add(base)
                    .ok_or(error!(FuturesError::MathOverflow))?;
                let new_quote = k
                    .checked_div(new_base)
                    .ok_or(error!(FuturesError::MathOverflow))?;
                let quote_out = self
                    .quote_reserve
                    .checked_sub(new_quote)
                    .ok_or(error!(FuturesError::MathOverflow))?;
                Ok((
                    u64::try_from(quote_out).map_err(|_| error!(FuturesError::MathOverflow))?,
                    new_base,
                    new_quote,
                ))
            }
            Side::Short => {
                // Buy base back: base_reserve falls, quote_reserve rises.
                require!(base < self.base_reserve, FuturesError::InsufficientLiquidity);
                let new_base = self
                    .base_reserve
                    .checked_sub(base)
                    .ok_or(error!(FuturesError::MathOverflow))?;
                let new_quote = k
                    .checked_div(new_base)
                    .ok_or(error!(FuturesError::MathOverflow))?;
                let quote_in = new_quote
                    .checked_sub(self.quote_reserve)
                    .ok_or(error!(FuturesError::MathOverflow))?;
                Ok((
                    u64::try_from(quote_in).map_err(|_| error!(FuturesError::MathOverflow))?,
                    new_base,
                    new_quote,
                ))
            }
        }
    }

    /// Commit a swap's reserves back onto the market.
    pub fn apply_reserves(&mut self, new_base: u128, new_quote: u128) {
        self.base_reserve = new_base;
        self.quote_reserve = new_quote;
    }

    /// A flat bps fee on a USDC notional. `notional * bps / 10_000`.
    pub fn fee_on(notional: u64, bps: u16) -> u64 {
        ((notional as u128)
            .saturating_mul(bps as u128)
            / BPS_DENOMINATOR) as u64
    }
}

// ─────────────────────────── Position ───────────────────────────

/// One trader's isolated position in one market. Isolated margin: the
/// `margin` here backs only this position and nothing else. A trader holds
/// at most one open position per market; increasing adds to it, closing
/// reduces it, and flipping sides requires a full close first.
#[account]
pub struct Position {
    /// PDA bump. Seeds: [b"position", market, owner].
    pub bump: u8,

    /// The market this position trades.
    pub market: Pubkey,

    /// The position owner (holder wallet).
    pub owner: Pubkey,

    /// USDC collateral committed to this position (6dp). Deposits raise it,
    /// withdrawals and fees lower it, realized losses consume it and realized
    /// profits are paid out of the collateral vault (not added here).
    pub margin: u64,

    /// Net base exposure (signed: +long, -short). Same 1e6 scale as price.
    pub base_size: i128,

    /// Cost basis: cumulative quote (USDC) swapped to build `base_size`.
    /// For a long this is USDC paid in; for a short, USDC received. PnL is
    /// measured against this.
    pub open_notional: u64,

    /// Lifetime realized PnL for this position (signed), for reporting.
    pub realized_pnl: i64,

    /// Unix ts of the last mutation.
    pub last_updated: i64,

    /// Reserved for forward-compatible upgrades.
    pub _reserved: [u8; 64],
}

impl Position {
    /// 8 (disc) + 1 (bump) + 32 (market) + 32 (owner) + 8 (margin)
    /// + 16 (base_size i128) + 8 (open_notional) + 8 (realized_pnl i64)
    /// + 8 (last_updated) + 64 (reserved).
    pub const MAX_SIZE: usize = 8 + 1 + 32 + 32 + 8 + 16 + 8 + 8 + 8 + 64;

    pub fn is_flat(&self) -> bool {
        self.base_size == 0
    }

    /// The side of the current position, or None when flat.
    pub fn side(&self) -> Option<Side> {
        if self.base_size > 0 {
            Some(Side::Long)
        } else if self.base_size < 0 {
            Some(Side::Short)
        } else {
            None
        }
    }

    /// Absolute base size as u128.
    pub fn abs_base(&self) -> u128 {
        self.base_size.unsigned_abs()
    }

    /// Position notional valued at a given price (1e6): |base| * price / 1e6.
    /// Used for margin ratios and the OI/leverage checks.
    pub fn notional_at(&self, price: u64) -> Result<u64> {
        let v = self
            .abs_base()
            .checked_mul(price as u128)
            .ok_or(error!(FuturesError::MathOverflow))?
            / PRICE_PRECISION;
        u64::try_from(v).map_err(|_| error!(FuturesError::MathOverflow))
    }

    /// Unrealized PnL (signed, USDC 6dp) if the position were valued at
    /// `price`. Long profits as price rises above the entry embedded in
    /// `open_notional`; short profits as it falls.
    pub fn unrealized_pnl(&self, price: u64) -> Result<i128> {
        if self.is_flat() {
            return Ok(0);
        }
        let value_now = self.notional_at(price)? as i128;
        let cost = self.open_notional as i128;
        Ok(match self.side() {
            Some(Side::Long) => value_now - cost,
            Some(Side::Short) => cost - value_now,
            None => 0,
        })
    }

    /// Account equity at `price`: margin plus unrealized PnL. May be negative
    /// (a position underwater past its margin).
    pub fn equity(&self, price: u64) -> Result<i128> {
        Ok(self.margin as i128 + self.unrealized_pnl(price)?)
    }

    /// Maintenance margin required at `price`: notional * mmr_bps / 10_000.
    pub fn maintenance_margin(&self, price: u64, mmr_bps: u16) -> Result<u128> {
        Ok((self.notional_at(price)? as u128).saturating_mul(mmr_bps as u128) / BPS_DENOMINATOR)
    }

    /// A position is liquidatable when equity falls below the maintenance
    /// margin required to hold it.
    pub fn is_liquidatable(&self, price: u64, mmr_bps: u16) -> Result<bool> {
        Ok(self.equity(price)? < self.maintenance_margin(price, mmr_bps)? as i128)
    }
}
