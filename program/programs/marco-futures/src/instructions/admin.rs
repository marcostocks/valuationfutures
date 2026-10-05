use anchor_lang::prelude::*;

use crate::errors::FuturesError;
use crate::state::{Market, MarketStatus, FuturesConfig};

/// Flip the protocol-wide pause. When paused, no market accepts new opens;
/// closing, liquidation and settlement stay available. Admin only.
pub fn pause(ctx: Context<AdminConfig>, paused: bool) -> Result<()> {
    let config = &mut ctx.accounts.config;
    config.paused = paused;
    msg!("Protocol paused = {}", paused);
    Ok(())
}

/// Rotate the operator (keeper) wallet. Admin only.
pub fn set_operator(ctx: Context<AdminConfig>, new_operator: Pubkey) -> Result<()> {
    let config = &mut ctx.accounts.config;
    config.operator = new_operator;
    msg!("Operator set to {}", new_operator);
    Ok(())
}

/// Halt or resume a single market. Halting stops new opens on that market;
/// resuming returns it to Active. A Settled market cannot be resumed — its
/// lifecycle is over. Admin only.
pub fn halt_market(ctx: Context<AdminMarket>, halted: bool) -> Result<()> {
    let market = &mut ctx.accounts.market;
    require!(
        market.status != MarketStatus::Settled,
        FuturesError::MarketAlreadySettled
    );
    market.status = if halted {
        MarketStatus::Halted
    } else {
        MarketStatus::Active
    };
    msg!("Market {} halted = {}", market.market_id, halted);
    Ok(())
}

#[derive(Accounts)]
pub struct AdminConfig<'info> {
    #[account(
        mut,
        seeds = [b"config"],
        bump = config.bump,
        has_one = admin @ FuturesError::UnauthorizedAdmin
    )]
    pub config: Account<'info, FuturesConfig>,

    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct AdminMarket<'info> {
    #[account(
        seeds = [b"config"],
        bump = config.bump,
        has_one = admin @ FuturesError::UnauthorizedAdmin
    )]
    pub config: Account<'info, FuturesConfig>,

    #[account(
        mut,
        seeds = [b"market", market.config.as_ref(), market.market_id.as_bytes()],
        bump = market.bump,
        constraint = market.config == config.key()
    )]
    pub market: Box<Account<'info, Market>>,

    pub admin: Signer<'info>,
}
