use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

use crate::errors::FuturesError;
use crate::events::CollateralWithdrawn;
use crate::state::{Market, MarketStatus, Position};

/// Withdraw free USDC margin — the collateral above the position's initial
/// margin requirement, valued at the current mark. A flat position can pull
/// all of it; an open one must leave enough to keep leverage within the
/// market maximum. Holder-signed; the market PDA authorises the payout.
///
/// The payout is clamped to the collateral vault's actual balance (solvency
/// by construction): the program never transfers more USDC than it holds.
pub fn handler(ctx: Context<WithdrawCollateral>, amount: u64) -> Result<()> {
    require!(amount > 0, FuturesError::ZeroAmount);
    require!(
        matches!(
            ctx.accounts.market.status,
            MarketStatus::Active | MarketStatus::Halted
        ),
        FuturesError::InvalidMarketStatus
    );

    let market_ai = ctx.accounts.market.to_account_info();
    let token_program_ai = ctx.accounts.token_program.to_account_info();
    let collateral_vault_ai = ctx.accounts.collateral_vault.to_account_info();
    let owner_usdc_ai = ctx.accounts.owner_usdc.to_account_info();

    let market_key = ctx.accounts.market.key();
    let owner_key = ctx.accounts.owner.key();
    let market_id = ctx.accounts.market.market_id.clone();
    let config_key = ctx.accounts.market.config;
    let bump = ctx.accounts.market.bump;
    let max_leverage = ctx.accounts.market.max_leverage;
    let vault_bal = ctx.accounts.collateral_vault.amount;

    // How much margin is free to withdraw.
    let withdrawable: u64 = {
        let position = &ctx.accounts.position;
        if position.is_flat() {
            position.margin
        } else {
            let mark = ctx.accounts.market.mark_price()?;
            let notional = position.notional_at(mark)?;
            // Initial margin requirement = ceil(notional / max_leverage).
            let required_initial = notional.div_ceil(max_leverage as u64);
            let equity = position.equity(mark)?; // margin + unrealized PnL
            let free = equity - required_initial as i128;
            if free <= 0 {
                0
            } else {
                (free as u128).min(position.margin as u128) as u64
            }
        }
    };
    require!(amount <= withdrawable, FuturesError::WithdrawBelowMargin);

    // Solvency: never pay out more than the vault actually holds.
    let payout = amount.min(vault_bal);
    require!(payout > 0, FuturesError::NothingToWithdraw);

    let seeds: &[&[u8]] = &[b"market", config_key.as_ref(), market_id.as_bytes(), &[bump]];
    let signer = &[seeds];

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

    let position = &mut ctx.accounts.position;
    position.margin = position.margin.checked_sub(payout).ok_or(FuturesError::MathOverflow)?;
    position.last_updated = Clock::get()?.unix_timestamp;
    let margin_after = position.margin;

    let market = &mut ctx.accounts.market;
    market.total_collateral = market.total_collateral.saturating_sub(payout);

    emit!(CollateralWithdrawn {
        market: market_key,
        market_id,
        owner: owner_key,
        amount: payout,
        margin_after,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct WithdrawCollateral<'info> {
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

    #[account(
        mut,
        constraint = owner_usdc.owner == owner.key(),
        constraint = owner_usdc.mint == market.usdc_mint @ FuturesError::WrongMint
    )]
    pub owner_usdc: Box<Account<'info, TokenAccount>>,

    pub owner: Signer<'info>,

    pub token_program: Program<'info, Token>,
}
