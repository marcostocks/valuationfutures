use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};

use crate::errors::FuturesError;
use crate::state::{Market, MarketStatus, FuturesConfig, BPS_DENOMINATOR, PRICE_PRECISION};

/// All market parameters, frozen at creation. `quote_reserve` sets the vAMM's
/// depth (in USDC-equivalent units); the base reserve is derived so the mark
/// price equals `anchor_price` exactly at launch.
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct MarketParams {
    pub market_id: String,
    /// Seed valuation index (1e6). e.g. a $12.5B anchor is 12_500_000.
    pub anchor_price: u64,
    /// vAMM quote-reserve depth (USDC-equivalent). Larger == less slippage.
    pub quote_reserve: u128,
    /// Max leverage, e.g. 3 == 3x.
    pub max_leverage: u8,
    /// Maintenance margin ratio in bps (must be below the initial ratio).
    pub maintenance_margin_bps: u16,
    /// Taker fee in bps on notional, charged at open and close.
    pub taker_fee_bps: u16,
    /// Liquidation penalty in bps of liquidated notional.
    pub liquidation_fee_bps: u16,
    /// Per-side open-interest cap in USDC notional (0 == uncapped).
    pub max_oi_notional: u64,
    /// Informational expected-listing timestamp / dated-expiry fallback.
    pub expiry_ts: i64,
}

pub fn handler(ctx: Context<CreateMarket>, p: MarketParams) -> Result<()> {
    require!(
        ctx.accounts.config.admin == ctx.accounts.admin.key(),
        FuturesError::UnauthorizedAdmin
    );
    require!(
        !p.market_id.is_empty() && p.market_id.len() <= Market::MAX_ID_LEN,
        FuturesError::MarketIdTooLong
    );
    require!(p.anchor_price > 0, FuturesError::InvalidParameter);
    require!(p.quote_reserve > 0, FuturesError::InvalidParameter);
    require!(
        p.max_leverage >= 1 && p.max_leverage <= Market::MAX_LEVERAGE_CAP,
        FuturesError::InvalidParameter
    );
    require!(p.taker_fee_bps <= Market::MAX_BPS, FuturesError::InvalidParameter);
    require!(
        p.liquidation_fee_bps <= Market::MAX_BPS,
        FuturesError::InvalidParameter
    );

    // Maintenance margin must sit strictly below the initial margin ratio
    // (1 / max_leverage), or a position could open already liquidatable.
    let initial_margin_bps = BPS_DENOMINATOR / (p.max_leverage as u128);
    require!(
        p.maintenance_margin_bps > 0 && (p.maintenance_margin_bps as u128) < initial_margin_bps,
        FuturesError::InvalidParameter
    );

    // base_reserve = quote_reserve * PRICE_PRECISION / anchor_price, so
    // mark == anchor at creation.
    let base_reserve = p
        .quote_reserve
        .checked_mul(PRICE_PRECISION)
        .ok_or(FuturesError::MathOverflow)?
        .checked_div(p.anchor_price as u128)
        .ok_or(FuturesError::MathOverflow)?;
    require!(base_reserve > 0, FuturesError::InvalidParameter);

    let now = Clock::get()?.unix_timestamp;

    let market = &mut ctx.accounts.market;
    market.bump = ctx.bumps.market;
    market.config = ctx.accounts.config.key();
    market.market_id = p.market_id;
    market.usdc_mint = ctx.accounts.usdc_mint.key();
    market.collateral_vault = ctx.accounts.collateral_vault.key();
    market.insurance_vault = ctx.accounts.insurance_vault.key();
    market.status = MarketStatus::Active;

    market.base_reserve = base_reserve;
    market.quote_reserve = p.quote_reserve;
    market.initial_base_reserve = base_reserve;
    market.initial_quote_reserve = p.quote_reserve;
    market.anchor_price = p.anchor_price;

    market.max_leverage = p.max_leverage;
    market.maintenance_margin_bps = p.maintenance_margin_bps;
    market.taker_fee_bps = p.taker_fee_bps;
    market.liquidation_fee_bps = p.liquidation_fee_bps;
    market.max_oi_notional = p.max_oi_notional;

    market.long_open_notional = 0;
    market.short_open_notional = 0;
    market.total_collateral = 0;
    market.position_count = 0;

    market.settlement_price = 0;
    market.expiry_ts = p.expiry_ts;
    market.settled_at = 0;
    market.created_at = now;
    market._reserved = [0u8; 128];

    let cfg = &mut ctx.accounts.config;
    cfg.market_count = cfg.market_count.checked_add(1).ok_or(FuturesError::MathOverflow)?;

    msg!(
        "Market {} | anchor {} | depth(q) {} | maxLev {}x | mmr {} bps | takerFee {} bps",
        market.market_id,
        market.anchor_price,
        market.quote_reserve,
        market.max_leverage,
        market.maintenance_margin_bps,
        market.taker_fee_bps
    );
    Ok(())
}

#[derive(Accounts)]
#[instruction(p: MarketParams)]
pub struct CreateMarket<'info> {
    #[account(mut, seeds = [b"config"], bump = config.bump)]
    pub config: Account<'info, FuturesConfig>,

    #[account(
        init,
        payer = admin,
        space = Market::MAX_SIZE,
        seeds = [b"market", config.key().as_ref(), p.market_id.as_bytes()],
        bump
    )]
    pub market: Account<'info, Market>,

    #[account(constraint = usdc_mint.key() == config.usdc_mint @ FuturesError::WrongMint)]
    pub usdc_mint: Account<'info, Mint>,

    /// Program-owned USDC account holding all trader margin for this market.
    #[account(
        init,
        payer = admin,
        seeds = [b"collateral", market.key().as_ref()],
        bump,
        token::mint = usdc_mint,
        token::authority = market
    )]
    pub collateral_vault: Account<'info, TokenAccount>,

    /// Program-owned USDC account backstopping this market.
    #[account(
        init,
        payer = admin,
        seeds = [b"insurance", market.key().as_ref()],
        bump,
        token::mint = usdc_mint,
        token::authority = market
    )]
    pub insurance_vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub admin: Signer<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}
