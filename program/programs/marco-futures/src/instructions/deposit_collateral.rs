use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

use crate::errors::FuturesError;
use crate::events::CollateralDeposited;
use crate::state::{Market, MarketStatus, Position};

/// Deposit USDC margin into the caller's isolated position for this market,
/// creating the position on first deposit. Holder-signed: the USDC is pulled
/// from the caller's own token account into the market collateral vault.
///
/// Allowed while the market is Active or Halted (a trader may always add
/// margin to defend a position); a Settled market takes no new margin.
pub fn handler(ctx: Context<DepositCollateral>, amount: u64) -> Result<()> {
    require!(amount > 0, FuturesError::ZeroAmount);
    require!(
        matches!(
            ctx.accounts.market.status,
            MarketStatus::Active | MarketStatus::Halted
        ),
        FuturesError::InvalidMarketStatus
    );

    let now = Clock::get()?.unix_timestamp;
    let market_key = ctx.accounts.market.key();
    let owner_key = ctx.accounts.owner.key();
    let market_id = ctx.accounts.market.market_id.clone();

    // Pull the margin into the collateral vault.
    token::transfer(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.owner_usdc.to_account_info(),
                to: ctx.accounts.collateral_vault.to_account_info(),
                authority: ctx.accounts.owner.to_account_info(),
            },
        ),
        amount,
    )?;

    // Initialise the position on first touch; a fresh account is zeroed, so
    // `market == default` distinguishes it from an existing one.
    let position = &mut ctx.accounts.position;
    if position.market == Pubkey::default() {
        position.bump = ctx.bumps.position;
        position.market = market_key;
        position.owner = owner_key;
        position.base_size = 0;
        position.open_notional = 0;
        position.realized_pnl = 0;
        position._reserved = [0u8; 64];
    }
    position.margin = position.margin.checked_add(amount).ok_or(FuturesError::MathOverflow)?;
    position.last_updated = now;
    let margin_after = position.margin;

    let market = &mut ctx.accounts.market;
    market.total_collateral = market
        .total_collateral
        .checked_add(amount)
        .ok_or(FuturesError::MathOverflow)?;

    emit!(CollateralDeposited {
        market: market_key,
        market_id,
        owner: owner_key,
        amount,
        margin_after,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct DepositCollateral<'info> {
    #[account(
        mut,
        seeds = [b"market", market.config.as_ref(), market.market_id.as_bytes()],
        bump = market.bump
    )]
    pub market: Box<Account<'info, Market>>,

    #[account(
        init_if_needed,
        payer = owner,
        space = Position::MAX_SIZE,
        seeds = [b"position", market.key().as_ref(), owner.key().as_ref()],
        bump
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

    #[account(mut)]
    pub owner: Signer<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}
