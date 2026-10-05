use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

use crate::errors::FuturesError;
use crate::events::PositionOpened;
use crate::state::{Market, MarketStatus, FuturesConfig, Position, Side, PRICE_PRECISION};

/// Open or increase a leveraged position against the vAMM.
///
/// `notional` USDC of exposure is swapped into the vAMM on `side`; the trader
/// receives the resulting base delta and their `open_notional` (cost basis)
/// grows by `notional`. The taker fee is taken from margin and routed to the
/// insurance vault. Flipping sides in one call is refused — close first.
///
/// Two guards bound risk: the resulting position valued at the post-swap mark
/// may not exceed `margin * max_leverage`, and per-side open interest may not
/// exceed the market cap.
pub fn handler(
    ctx: Context<OpenPosition>,
    is_long: bool,
    notional: u64,
    limit_price: u64,
) -> Result<()> {
    require!(!ctx.accounts.config.paused, FuturesError::ProtocolPaused);
    require!(notional > 0, FuturesError::ZeroAmount);
    ctx.accounts.market.require_status(MarketStatus::Active)?;

    let side = if is_long { Side::Long } else { Side::Short };

    // AccountInfos captured up front for the fee CPI (no field borrow held).
    let market_ai = ctx.accounts.market.to_account_info();
    let token_program_ai = ctx.accounts.token_program.to_account_info();
    let collateral_vault_ai = ctx.accounts.collateral_vault.to_account_info();
    let insurance_vault_ai = ctx.accounts.insurance_vault.to_account_info();

    let market_key = ctx.accounts.market.key();
    let owner_key = ctx.accounts.owner.key();
    let market_id = ctx.accounts.market.market_id.clone();
    let config_key = ctx.accounts.market.config;
    let bump = ctx.accounts.market.bump;
    let taker_fee_bps = ctx.accounts.market.taker_fee_bps;
    let max_leverage = ctx.accounts.market.max_leverage;
    let max_oi = ctx.accounts.market.max_oi_notional;

    // No side flips in one instruction.
    if let Some(cur) = ctx.accounts.position.side() {
        require!(cur == side, FuturesError::WrongSide);
    }

    // Fee comes off margin; the rest must still back the position.
    let fee = Market::fee_on(notional, taker_fee_bps);
    let cur_margin = ctx.accounts.position.margin;
    let margin_after_fee = cur_margin.checked_sub(fee).ok_or(FuturesError::InsufficientMargin)?;
    require!(margin_after_fee > 0, FuturesError::InsufficientMargin);

    // Swap against the current reserves (pure — reserves committed below).
    let sr = ctx.accounts.market.quote_open(side, notional)?;
    let base_abs = sr.base_delta.unsigned_abs();
    require!(base_abs > 0, FuturesError::ZeroAmount);

    // Average fill price and slippage guard.
    let avg_price = u64::try_from(
        (notional as u128)
            .checked_mul(PRICE_PRECISION)
            .ok_or(FuturesError::MathOverflow)?
            / base_abs,
    )
    .map_err(|_| FuturesError::MathOverflow)?;
    if limit_price > 0 {
        match side {
            Side::Long => require!(avg_price <= limit_price, FuturesError::SlippageExceeded),
            Side::Short => require!(avg_price >= limit_price, FuturesError::SlippageExceeded),
        }
    }

    let was_flat = ctx.accounts.position.is_flat();
    let new_base = ctx
        .accounts
        .position
        .base_size
        .checked_add(sr.base_delta)
        .ok_or(FuturesError::MathOverflow)?;
    let new_open_notional = ctx
        .accounts
        .position
        .open_notional
        .checked_add(notional)
        .ok_or(FuturesError::MathOverflow)?;

    // Mark after the swap, and the resulting leverage check.
    let mark_after = u64::try_from(
        sr.new_quote_reserve
            .checked_mul(PRICE_PRECISION)
            .ok_or(FuturesError::MathOverflow)?
            / sr.new_base_reserve,
    )
    .map_err(|_| FuturesError::MathOverflow)?;
    let new_notional_value = u64::try_from(
        new_base
            .unsigned_abs()
            .checked_mul(mark_after as u128)
            .ok_or(FuturesError::MathOverflow)?
            / PRICE_PRECISION,
    )
    .map_err(|_| FuturesError::MathOverflow)?;
    require!(
        (new_notional_value as u128) <= (margin_after_fee as u128).saturating_mul(max_leverage as u128),
        FuturesError::ExceedsMaxLeverage
    );

    // Per-side open-interest cap.
    if max_oi > 0 {
        let side_oi_after = match side {
            Side::Long => ctx
                .accounts
                .market
                .long_open_notional
                .checked_add(notional)
                .ok_or(FuturesError::MathOverflow)?,
            Side::Short => ctx
                .accounts
                .market
                .short_open_notional
                .checked_add(notional)
                .ok_or(FuturesError::MathOverflow)?,
        };
        require!(side_oi_after <= max_oi, FuturesError::OpenInterestCapExceeded);
    }

    // Route the taker fee to the insurance vault (market PDA signs).
    if fee > 0 {
        let seeds: &[&[u8]] = &[b"market", config_key.as_ref(), market_id.as_bytes(), &[bump]];
        let signer = &[seeds];
        token::transfer(
            CpiContext::new_with_signer(
                token_program_ai,
                Transfer {
                    from: collateral_vault_ai,
                    to: insurance_vault_ai,
                    authority: market_ai,
                },
                signer,
            ),
            fee,
        )?;
    }

    // Commit market state.
    {
        let market = &mut ctx.accounts.market;
        market.apply_reserves(sr.new_base_reserve, sr.new_quote_reserve);
        match side {
            Side::Long => {
                market.long_open_notional = market
                    .long_open_notional
                    .checked_add(notional)
                    .ok_or(FuturesError::MathOverflow)?
            }
            Side::Short => {
                market.short_open_notional = market
                    .short_open_notional
                    .checked_add(notional)
                    .ok_or(FuturesError::MathOverflow)?
            }
        }
        market.total_collateral = market.total_collateral.saturating_sub(fee);
        if was_flat {
            market.position_count = market
                .position_count
                .checked_add(1)
                .ok_or(FuturesError::MathOverflow)?;
        }
    }

    // Commit position state.
    let now = Clock::get()?.unix_timestamp;
    {
        let position = &mut ctx.accounts.position;
        position.base_size = new_base;
        position.open_notional = new_open_notional;
        position.margin = margin_after_fee;
        position.last_updated = now;
    }

    emit!(PositionOpened {
        market: market_key,
        market_id,
        owner: owner_key,
        is_long,
        notional,
        base_delta: base_abs,
        fee,
        mark_price: mark_after,
        base_size_after: new_base,
        margin_after: margin_after_fee,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct OpenPosition<'info> {
    #[account(seeds = [b"config"], bump = config.bump)]
    pub config: Account<'info, FuturesConfig>,

    #[account(
        mut,
        seeds = [b"market", market.config.as_ref(), market.market_id.as_bytes()],
        bump = market.bump,
        constraint = market.config == config.key()
    )]
    pub market: Box<Account<'info, Market>>,

    #[account(
        mut,
        has_one = owner @ FuturesError::UnauthorizedOwner,
        constraint = position.market == market.key(),
        seeds = [b"position", market.key().as_ref(), owner.key().as_ref()],
        bump = position.bump
    )]
    pub position: Box<Account<'info, Position>>,

    #[account(mut, constraint = collateral_vault.key() == market.collateral_vault @ FuturesError::WrongCollateralVault)]
    pub collateral_vault: Box<Account<'info, TokenAccount>>,

    #[account(mut, constraint = insurance_vault.key() == market.insurance_vault @ FuturesError::WrongInsuranceVault)]
    pub insurance_vault: Box<Account<'info, TokenAccount>>,

    pub owner: Signer<'info>,

    pub token_program: Program<'info, Token>,
}
