use anchor_lang::prelude::*;

use crate::errors::FuturesError;
use crate::events::MarketSettled;
use crate::state::{Market, MarketStatus, FuturesConfig};

/// Post the final valuation at the real listing / acquisition event and move
/// the market to Settled. Admin or operator only. After this, no trading:
/// holders settle each position at `settlement_price` via `settle_position`.
///
/// This is the dated-future settlement — an exogenous price, not a vAMM close,
/// so the last trader out doesn't eat the pool's slippage.
pub fn handler(ctx: Context<SettleMarket>, settlement_price: u64) -> Result<()> {
    let signer = ctx.accounts.authority.key();
    require!(
        signer == ctx.accounts.config.admin || signer == ctx.accounts.config.operator,
        FuturesError::UnauthorizedOperator
    );
    require!(settlement_price > 0, FuturesError::InvalidParameter);
    require!(
        ctx.accounts.market.status != MarketStatus::Settled,
        FuturesError::MarketAlreadySettled
    );

    let now = Clock::get()?.unix_timestamp;
    let market = &mut ctx.accounts.market;
    market.settlement_price = settlement_price;
    market.status = MarketStatus::Settled;
    market.settled_at = now;

    msg!(
        "Market {} settled at {} (1e6 index)",
        market.market_id,
        settlement_price
    );

    emit!(MarketSettled {
        market: market.key(),
        market_id: market.market_id.clone(),
        settlement_price,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct SettleMarket<'info> {
    #[account(seeds = [b"config"], bump = config.bump)]
    pub config: Account<'info, FuturesConfig>,

    #[account(
        mut,
        seeds = [b"market", market.config.as_ref(), market.market_id.as_bytes()],
        bump = market.bump,
        constraint = market.config == config.key()
    )]
    pub market: Box<Account<'info, Market>>,

    pub authority: Signer<'info>,
}
