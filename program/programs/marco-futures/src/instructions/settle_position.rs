use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

use crate::errors::FuturesError;
use crate::events::PositionSettled;
use crate::state::{Market, MarketStatus, Position, Side};

/// Settle one position at the market's final price. Permissionless — anyone may
/// trigger it; the payout always goes to the position's owner. PnL is computed
/// at the fixed `settlement_price` (no vAMM slippage). The owner receives
/// `margin + PnL`, floored at zero and clamped to available USDC; when the
/// collateral vault is short, the insurance fund tops it up so winners are
/// paid before the fund is exhausted.
pub fn handler(ctx: Context<SettlePosition>) -> Result<()> {
    ctx.accounts.market.require_status(MarketStatus::Settled)?;

    let side = ctx.accounts.position.side().ok_or(FuturesError::PositionFlat)?;
    let settle_price = ctx.accounts.market.settlement_price;
    let old_margin = ctx.accounts.position.margin;
    let open_notional = ctx.accounts.position.open_notional;

    let market_ai = ctx.accounts.market.to_account_info();
    let token_program_ai = ctx.accounts.token_program.to_account_info();
    let collateral_vault_ai = ctx.accounts.collateral_vault.to_account_info();
    let insurance_vault_ai = ctx.accounts.insurance_vault.to_account_info();
    let owner_usdc_ai = ctx.accounts.owner_usdc.to_account_info();

    let market_key = ctx.accounts.market.key();
    let owner_key = ctx.accounts.position.owner;
    let market_id = ctx.accounts.market.market_id.clone();
    let config_key = ctx.accounts.market.config;
    let bump = ctx.accounts.market.bump;

    let vault_bal = ctx.accounts.collateral_vault.amount;
    let insurance_bal = ctx.accounts.insurance_vault.amount;

    // PnL at the fixed settlement price, and equity owed.
    let pnl = ctx.accounts.position.unrealized_pnl(settle_price)?;
    let equity_i = old_margin as i128 + pnl;
    let payout_owed: u64 = if equity_i > 0 {
        u64::try_from(equity_i).unwrap_or(u64::MAX)
    } else {
        0
    };

    // Top up from insurance if the pool is short of what this winner is owed.
    let insurance_draw: u64 = if payout_owed > vault_bal {
        payout_owed.saturating_sub(vault_bal).min(insurance_bal)
    } else {
        0
    };
    let available = vault_bal.saturating_add(insurance_draw);
    let payout = payout_owed.min(available);

    {
        let seeds: &[&[u8]] = &[b"market", config_key.as_ref(), market_id.as_bytes(), &[bump]];
        let signer = &[seeds];
        if insurance_draw > 0 {
            token::transfer(
                CpiContext::new_with_signer(
                    token_program_ai.clone(),
                    Transfer {
                        from: insurance_vault_ai,
                        to: collateral_vault_ai.clone(),
                        authority: market_ai.clone(),
                    },
                    signer,
                ),
                insurance_draw,
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

    {
        let market = &mut ctx.accounts.market;
        match side {
            Side::Long => {
                market.long_open_notional = market.long_open_notional.saturating_sub(open_notional)
            }
            Side::Short => {
                market.short_open_notional = market.short_open_notional.saturating_sub(open_notional)
            }
        }
        market.total_collateral = market.total_collateral.saturating_sub(old_margin);
        market.position_count = market.position_count.saturating_sub(1);
    }

    {
        let position = &mut ctx.accounts.position;
        position.base_size = 0;
        position.open_notional = 0;
        position.margin = 0;
        let pnl_i64 = i64::try_from(pnl).unwrap_or(if pnl < 0 { i64::MIN } else { i64::MAX });
        position.realized_pnl = position.realized_pnl.saturating_add(pnl_i64);
        position.last_updated = now;
    }

    emit!(PositionSettled {
        market: market_key,
        market_id,
        owner: owner_key,
        realized_pnl: i64::try_from(pnl).unwrap_or(if pnl < 0 { i64::MIN } else { i64::MAX }),
        payout,
        insurance_draw,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct SettlePosition<'info> {
    #[account(
        mut,
        seeds = [b"market", market.config.as_ref(), market.market_id.as_bytes()],
        bump = market.bump
    )]
    pub market: Box<Account<'info, Market>>,

    #[account(
        mut,
        constraint = position.market == market.key(),
        seeds = [b"position", market.key().as_ref(), position.owner.as_ref()],
        bump = position.bump
    )]
    pub position: Box<Account<'info, Position>>,

    #[account(mut, constraint = collateral_vault.key() == market.collateral_vault @ FuturesError::WrongCollateralVault)]
    pub collateral_vault: Box<Account<'info, TokenAccount>>,

    #[account(mut, constraint = insurance_vault.key() == market.insurance_vault @ FuturesError::WrongInsuranceVault)]
    pub insurance_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = owner_usdc.owner == position.owner,
        constraint = owner_usdc.mint == market.usdc_mint @ FuturesError::WrongMint
    )]
    pub owner_usdc: Box<Account<'info, TokenAccount>>,

    pub caller: Signer<'info>,

    pub token_program: Program<'info, Token>,
}
