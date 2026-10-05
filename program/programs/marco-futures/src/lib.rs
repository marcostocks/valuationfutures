use anchor_lang::prelude::*;

pub mod errors;
pub mod events;
pub mod instructions;
pub mod state;

use instructions::*;

declare_id!("GCW6Gt86tSVMqEwz6GkVNDWuzivDXCG2bzjp4AS3ztkW");

/// Marco Valuation Futures.
///
/// One market per private company (e.g. Moonshot AI). Traders take
/// leveraged long/short exposure against a virtual AMM whose price is the
/// mark; at the real listing/acquisition the admin posts a settlement price
/// and every position settles at that fixed number. It is a DATED future —
/// no funding rate.
///
/// The program is the counterparty: trader USDC margin lives in a
/// program-owned collateral vault, and a per-market insurance fund backstops
/// the residual skew. Two invariants keep that safe — solvency by
/// construction (never pay out more than a vault holds) and per-market
/// isolation (margin, reserves and insurance never cross markets).
#[program]
pub mod marco_futures {
    use super::*;

    /// Create the protocol config. PDA seeds: [b"config"]. Admin only,
    /// once per deployment.
    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        instructions::initialize::handler(ctx)
    }

    /// Create a valuation-futures market. Seeds the vAMM so the mark equals
    /// the anchor valuation, and opens the collateral and insurance vaults.
    /// PDA seeds: [b"market", config, market_id]. Admin only.
    pub fn create_market(ctx: Context<CreateMarket>, params: MarketParams) -> Result<()> {
        instructions::create_market::handler(ctx, params)
    }

    /// Fund a market's insurance vault with USDC. Permissionless — anyone can
    /// strengthen the backstop.
    pub fn seed_insurance(ctx: Context<SeedInsurance>, amount: u64) -> Result<()> {
        instructions::seed_insurance::handler(ctx, amount)
    }

    /// Deposit USDC margin into the caller's position (created on first
    /// deposit). Holder-signed.
    pub fn deposit_collateral(ctx: Context<DepositCollateral>, amount: u64) -> Result<()> {
        instructions::deposit_collateral::handler(ctx, amount)
    }

    /// Withdraw free USDC margin — the amount above the position's initial
    /// margin requirement. Holder-signed.
    pub fn withdraw_collateral(ctx: Context<WithdrawCollateral>, amount: u64) -> Result<()> {
        instructions::withdraw_collateral::handler(ctx, amount)
    }

    /// Open or increase a leveraged position against the vAMM. `notional` is
    /// the USDC exposure to add; `limit_price` (1e6, 0 to skip) bounds the
    /// average fill. Holder-signed.
    pub fn open_position(
        ctx: Context<OpenPosition>,
        is_long: bool,
        notional: u64,
        limit_price: u64,
    ) -> Result<()> {
        instructions::open_position::handler(ctx, is_long, notional, limit_price)
    }

    /// Close or reduce a position against the vAMM. `base_amount` is the base
    /// to close (0 == close all); `limit_price` (1e6, 0 to skip) bounds the
    /// average fill. Realized PnL settles into margin; a full close pays the
    /// remaining margin back to the trader. Holder-signed.
    pub fn close_position(
        ctx: Context<ClosePosition>,
        base_amount: u128,
        limit_price: u64,
    ) -> Result<()> {
        instructions::close_position::handler(ctx, base_amount, limit_price)
    }

    /// Liquidate an under-collateralised position at the vAMM. Permissionless
    /// keeper: callable once equity, marked at the vAMM, falls below the
    /// maintenance margin. Pays the liquidator a reward and returns any
    /// residual equity to the owner; the insurance fund covers a deficit.
    pub fn liquidate_position(ctx: Context<LiquidatePosition>) -> Result<()> {
        instructions::liquidate_position::handler(ctx)
    }

    /// Post the final valuation at the real listing/acquisition and move the
    /// market to Settled. Admin or operator only.
    pub fn settle_market(ctx: Context<SettleMarket>, settlement_price: u64) -> Result<()> {
        instructions::settle_market::handler(ctx, settlement_price)
    }

    /// Settle one position at the market's final price and pay the owner.
    /// Permissionless; PnL is computed at the fixed settlement price.
    pub fn settle_position(ctx: Context<SettlePosition>) -> Result<()> {
        instructions::settle_position::handler(ctx)
    }

    /// Flip the protocol-wide pause (blocks new opens everywhere). Admin only.
    pub fn pause(ctx: Context<AdminConfig>, paused: bool) -> Result<()> {
        instructions::admin::pause(ctx, paused)
    }

    /// Rotate the operator (keeper) wallet. Admin only.
    pub fn set_operator(ctx: Context<AdminConfig>, new_operator: Pubkey) -> Result<()> {
        instructions::admin::set_operator(ctx, new_operator)
    }

    /// Halt or resume a single market (blocks new opens on it). Admin only.
    pub fn halt_market(ctx: Context<AdminMarket>, halted: bool) -> Result<()> {
        instructions::admin::halt_market(ctx, halted)
    }
}
