use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

use crate::errors::FuturesError;
use crate::events::PositionClosed;
use crate::state::{Market, MarketStatus, Position, Side, PRICE_PRECISION};

/// Close or reduce a position against the vAMM.
///
/// `base_amount` base units are swapped back (0 == close the whole position).
/// Realized PnL and the taker fee settle into the position's margin; on a full
/// close the remaining margin is paid back to the trader. On a partial close
/// the position keeps its (now-smaller) margin, withdrawable separately.
///
/// Solvency by construction: the fee and the payout are each clamped to the
/// collateral vault's live balance, so the program can never transfer out more
/// USDC than it holds. A profitable close is funded from the pooled margin of
/// losing positions; the insurance fund is the backstop of last resort.
pub fn handler(ctx: Context<ClosePosition>, base_amount: u128, limit_price: u64) -> Result<()> {
    require!(
        matches!(
            ctx.accounts.market.status,
            MarketStatus::Active | MarketStatus::Halted
        ),
        FuturesError::InvalidMarketStatus
    );

    let side = ctx.accounts.position.side().ok_or(FuturesError::PositionFlat)?;
    let abs_base = ctx.accounts.position.abs_base();
    let close_base = if base_amount == 0 || base_amount >= abs_base {
        abs_base
    } else {
        base_amount
    };
    require!(close_base > 0, FuturesError::ZeroAmount);

    // AccountInfos captured up front for the payout/fee CPIs.
    let market_ai = ctx.accounts.market.to_account_info();
    let token_program_ai = ctx.accounts.token_program.to_account_info();
    let collateral_vault_ai = ctx.accounts.collateral_vault.to_account_info();
    let insurance_vault_ai = ctx.accounts.insurance_vault.to_account_info();
    let owner_usdc_ai = ctx.accounts.owner_usdc.to_account_info();

    let market_key = ctx.accounts.market.key();
    let owner_key = ctx.accounts.owner.key();
    let market_id = ctx.accounts.market.market_id.clone();
    let config_key = ctx.accounts.market.config;
    let bump = ctx.accounts.market.bump;
    let taker_fee_bps = ctx.accounts.market.taker_fee_bps;
    let vault_bal = ctx.accounts.collateral_vault.amount;

    let old_margin = ctx.accounts.position.margin;
    let open_notional = ctx.accounts.position.open_notional;

    // Swap the base back out against the current reserves.
    let (quote, new_base_res, new_quote_res) = ctx.accounts.market.quote_close(side, close_base)?;

    // Average fill and slippage guard (a long close wants a high price; a
    // short close, which buys base back, wants a low one).
    let avg_price = u64::try_from(
        (quote as u128)
            .checked_mul(PRICE_PRECISION)
            .ok_or(FuturesError::MathOverflow)?
            / close_base,
    )
    .map_err(|_| FuturesError::MathOverflow)?;
    if limit_price > 0 {
        match side {
            Side::Long => require!(avg_price >= limit_price, FuturesError::SlippageExceeded),
            Side::Short => require!(avg_price <= limit_price, FuturesError::SlippageExceeded),
        }
    }

    // Cost basis released on the closed fraction, and the realized PnL.
    let closed_notional_cost = u64::try_from(
        (open_notional as u128)
            .checked_mul(close_base)
            .ok_or(FuturesError::MathOverflow)?
            / abs_base,
    )
    .map_err(|_| FuturesError::MathOverflow)?;
    let pnl: i128 = match side {
        Side::Long => quote as i128 - closed_notional_cost as i128,
        Side::Short => closed_notional_cost as i128 - quote as i128,
    };

    // Fee on the traded notional, taken from the settled margin.
    let fee = Market::fee_on(quote, taker_fee_bps);

    // Realize PnL and fee into margin; margin can't go below zero (a deeper
    // loss than margin is pool/insurance debt, not a trader receivable).
    let margin_i = old_margin as i128 + pnl - fee as i128;
    let margin_nonneg: u64 = if margin_i <= 0 {
        0
    } else {
        u64::try_from(margin_i).unwrap_or(u64::MAX)
    };

    let remaining_base_abs = abs_base.checked_sub(close_base).ok_or(FuturesError::MathOverflow)?;
    let fully_closed = remaining_base_abs == 0;
    let new_open_notional = open_notional
        .checked_sub(closed_notional_cost)
        .ok_or(FuturesError::MathOverflow)?;
    let new_base_size: i128 = match side {
        Side::Long => remaining_base_abs as i128,
        Side::Short => -(remaining_base_abs as i128),
    };

    // A full close pays remaining margin out; a partial close keeps it.
    let (final_margin, mut payout) = if fully_closed {
        (0u64, margin_nonneg)
    } else {
        (margin_nonneg, 0u64)
    };

    // Solvency clamps: fee first, then payout out of what remains.
    let fee_paid = fee.min(vault_bal);
    let bal_after_fee = vault_bal.saturating_sub(fee_paid);
    if payout > bal_after_fee {
        payout = bal_after_fee;
    }

    // Move USDC (market PDA signs both legs).
    {
        let seeds: &[&[u8]] = &[b"market", config_key.as_ref(), market_id.as_bytes(), &[bump]];
        let signer = &[seeds];
        if fee_paid > 0 {
            token::transfer(
                CpiContext::new_with_signer(
                    token_program_ai.clone(),
                    Transfer {
                        from: collateral_vault_ai.clone(),
                        to: insurance_vault_ai,
                        authority: market_ai.clone(),
                    },
                    signer,
                ),
                fee_paid,
            )?;
        }
        if payout > 0 {
            token::transfer(
                CpiContext::new_with_signer(
                    token_program_ai,
                    Transfer {
                        from: collateral_vault_ai,
                        to: owner_usdc_ai,
                        authority: market_ai,
                    },
                    signer,
                ),
                payout,
            )?;
        }
    }

    let now = Clock::get()?.unix_timestamp;

    // Commit market state.
    {
        let market = &mut ctx.accounts.market;
        market.apply_reserves(new_base_res, new_quote_res);
        match side {
            Side::Long => {
                market.long_open_notional =
                    market.long_open_notional.saturating_sub(closed_notional_cost)
            }
            Side::Short => {
                market.short_open_notional =
                    market.short_open_notional.saturating_sub(closed_notional_cost)
            }
        }
        // Mirror of summed margins: swap out the old margin, swap in the new.
        market.total_collateral = market
            .total_collateral
            .saturating_sub(old_margin)
            .saturating_add(final_margin);
        if fully_closed {
            market.position_count = market.position_count.saturating_sub(1);
        }
    }

    // Commit position state.
    let mark_after = ctx.accounts.market.mark_price()?;
    {
        let position = &mut ctx.accounts.position;
        position.base_size = new_base_size;
        position.open_notional = new_open_notional;
        position.margin = final_margin;
        let pnl_i64 = i64::try_from(pnl).unwrap_or(if pnl < 0 { i64::MIN } else { i64::MAX });
        position.realized_pnl = position.realized_pnl.saturating_add(pnl_i64);
        position.last_updated = now;
    }

    emit!(PositionClosed {
        market: market_key,
        market_id,
        owner: owner_key,
        base_delta: close_base,
        realized_pnl: i64::try_from(pnl).unwrap_or(if pnl < 0 { i64::MIN } else { i64::MAX }),
        fee: fee_paid,
        payout,
        mark_price: mark_after,
        base_size_after: new_base_size,
        margin_after: final_margin,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct ClosePosition<'info> {
    #[account(
        mut,
        seeds = [b"market", market.config.as_ref(), market.market_id.as_bytes()],
        bump = market.bump
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

    #[account(
        mut,
        constraint = owner_usdc.owner == owner.key(),
        constraint = owner_usdc.mint == market.usdc_mint @ FuturesError::WrongMint
    )]
    pub owner_usdc: Box<Account<'info, TokenAccount>>,

    pub owner: Signer<'info>,

    pub token_program: Program<'info, Token>,
}
