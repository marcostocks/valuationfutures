use anchor_lang::prelude::*;

/// Events for the holder-initiated instructions, mirroring the vault program's
/// convention: the off-chain orchestrator builds every intent from an observed
/// chain event rather than an HTTP call, so each event carries enough to act on
/// without re-reading account state. `market_id` rides alongside the market
/// pubkey because the orchestrator keys markets by that string.
///
/// Admin transitions (initialize, create_market, halt, settle_market) are not
/// emitted — the operator submits those and already knows they happened.
/// Liquidation IS emitted: it can be triggered permissionlessly by a third
/// party, so it arrives at the orchestrator unannounced otherwise.

/// A trader added USDC margin to their position.
#[event]
pub struct CollateralDeposited {
    pub market: Pubkey,
    pub market_id: String,
    pub owner: Pubkey,
    pub amount: u64,
    pub margin_after: u64,
}

/// A trader withdrew free USDC margin from their position.
#[event]
pub struct CollateralWithdrawn {
    pub market: Pubkey,
    pub market_id: String,
    pub owner: Pubkey,
    pub amount: u64,
    pub margin_after: u64,
}

/// A trader opened or increased a position against the vAMM.
#[event]
pub struct PositionOpened {
    pub market: Pubkey,
    pub market_id: String,
    pub owner: Pubkey,
    /// True for a long, false for a short.
    pub is_long: bool,
    /// USDC notional swapped in on this fill.
    pub notional: u64,
    /// Base acquired on this fill (absolute value).
    pub base_delta: u128,
    /// Taker fee taken from margin and routed to the insurance vault.
    pub fee: u64,
    /// vAMM mark price after the fill (1e6).
    pub mark_price: u64,
    /// Position base size after the fill (signed).
    pub base_size_after: i128,
    pub margin_after: u64,
}

/// A trader closed or reduced a position against the vAMM.
#[event]
pub struct PositionClosed {
    pub market: Pubkey,
    pub market_id: String,
    pub owner: Pubkey,
    /// Base closed on this fill (absolute value).
    pub base_delta: u128,
    /// Realized PnL on the closed portion (signed, USDC 6dp).
    pub realized_pnl: i64,
    /// Taker fee routed to the insurance vault.
    pub fee: u64,
    /// USDC paid out to the trader on this close (0 if none was free).
    pub payout: u64,
    /// vAMM mark price after the fill (1e6).
    pub mark_price: u64,
    /// Position base size after the fill (signed; 0 when fully closed).
    pub base_size_after: i128,
    pub margin_after: u64,
}

/// A position was liquidated (permissionless keeper). The whole position is
/// closed at the vAMM; `reward` goes to the liquidator, any remaining equity
/// to the owner, and `insurance_cover` is drawn from the insurance fund to
/// make the pool whole when the position closed underwater (`bad_debt`).
#[event]
pub struct PositionLiquidated {
    pub market: Pubkey,
    pub market_id: String,
    pub owner: Pubkey,
    pub liquidator: Pubkey,
    /// Base closed (absolute value; always the full position).
    pub base_closed: u128,
    /// Realized PnL at liquidation (signed, USDC 6dp).
    pub realized_pnl: i64,
    /// Keeper reward paid to the liquidator.
    pub reward: u64,
    /// Residual equity returned to the owner (0 when underwater).
    pub trader_refund: u64,
    /// Equity deficit past the owner's margin (0 when solvent).
    pub bad_debt: u64,
    /// USDC drawn from the insurance fund to cover the deficit.
    pub insurance_cover: u64,
    /// The mark that triggered the liquidation (1e6).
    pub mark_price: u64,
}

/// The market settled: the admin/operator posted the final valuation. No more
/// trading; holders settle each position at `settlement_price`.
#[event]
pub struct MarketSettled {
    pub market: Pubkey,
    pub market_id: String,
    pub settlement_price: u64,
}

/// A holder's position settled at the market's final price.
#[event]
pub struct PositionSettled {
    pub market: Pubkey,
    pub market_id: String,
    pub owner: Pubkey,
    /// Realized PnL at the settlement price (signed, USDC 6dp).
    pub realized_pnl: i64,
    /// USDC paid out to the owner.
    pub payout: u64,
    /// USDC drawn from the insurance fund to fund the payout.
    pub insurance_draw: u64,
}
