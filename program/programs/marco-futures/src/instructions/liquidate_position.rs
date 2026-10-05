use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

use crate::errors::FuturesError;
use crate::events::PositionLiquidated;
use crate::state::{Market, MarketStatus, Position};

/// Liquidate an under-collateralised position. Permissionless: any keeper may
/// call it once the position's equity, marked at the vAMM, falls below its
/// maintenance margin. The whole position is closed at the vAMM.
///
/// Payout waterfall, all clamped to live balances (solvency by construction):
///  1. If the position closed underwater, draw the deficit from the insurance
///     fund into the collateral vault so the pool stays whole for others.
///  2. Pay the liquidator their `reward` (a fraction of notional) — the
///     keeper incentive.
///  3. Return any residual equity to the owner.
pub fn handler(ctx: Context<LiquidatePosition>) -> Result<()> {
    require!(
        matches!(
            ctx.accounts.market.status,
            MarketStatus::Active | MarketStatus::Halted
        ),
        FuturesError::InvalidMarketStatus
    );

    let mark = ctx.accounts.market.mark_price()?;
    let mmr_bps = ctx.accounts.market.maintenance_margin_bps;
    require!(
        ctx.accounts.position.is_liquidatable(mark, mmr_bps)?,
        FuturesError::NotLiquidatable
    );

    let side = ctx.accounts.position.side().ok_or(FuturesError::PositionFlat)?;
    let abs_base = ctx.accounts.position.abs_base();
    let old_margin = ctx.accounts.position.margin;
    let open_notional = ctx.accounts.position.open_notional;

    // AccountInfos for the CPIs.
    let market_ai = ctx.accounts.market.to_account_info();
    let token_program_ai = ctx.accounts.token_program.to_account_info();
    let collateral_vault_ai = ctx.accounts.collateral_vault.to_account_info();
    let insurance_vault_ai = ctx.accounts.insurance_vault.to_account_info();
    let owner_usdc_ai = ctx.accounts.owner_usdc.to_account_info();
    let liquidator_usdc_ai = ctx.accounts.liquidator_usdc.to_account_info();

    let market_key = ctx.accounts.market.key();
    let owner_key = ctx.accounts.position.owner;
    let liquidator_key = ctx.accounts.liquidator.key();
    let market_id = ctx.accounts.market.market_id.clone();
    let config_key = ctx.accounts.market.config;
    let bump = ctx.accounts.market.bump;
    let liq_fee_bps = ctx.accounts.market.liquidation_fee_bps;

    let vault_bal = ctx.accounts.collateral_vault.amount;
    let insurance_bal = ctx.accounts.insurance_vault.amount;

    // Close the whole position at the vAMM.
    let (quote, new_base_res, new_quote_res) = ctx.accounts.market.quote_close(side, abs_base)?;
    let pnl: i128 = match side {
        crate::state::Side::Long => quote as i128 - open_notional as i128,
        crate::state::Side::Short => open_notional as i128 - quote as i128,
    };
    let equity_i: i128 = old_margin as i128 + pnl;

    // Keeper reward: a fraction of the closed notional.
    let reward = Market::fee_on(quote, liq_fee_bps);

    // Cover any deficit (negative equity) from insurance so the pool is whole.
    let bad_debt: u64 = if equity_i < 0 {
        u64::try_from(-equity_i).unwrap_or(u64::MAX)
    } else {
        0
    };
    let insurance_cover = bad_debt.min(insurance_bal);
    let mut available = vault_bal.saturating_add(insurance_cover);

    // Residual equity owed to the owner after the reward.
    let trader_refund_owed: u64 = {
        let after_reward = equity_i.checked_sub(reward as i128).ok_or(FuturesError::MathOverflow)?;
        if after_reward > 0 {
            u64::try_from(after_reward).unwrap_or(u64::MAX)
        } else {
            0
        }
    };

    let reward_paid = reward.min(available);
    available = available.saturating_sub(reward_paid);
    let trader_refund = trader_refund_owed.min(available);

    // Move USDC (market PDA signs every leg).
    {
        let seeds: &[&[u8]] = &[b"market", config_key.as_ref(), market_id.as_bytes(), &[bump]];
        let signer = &[seeds];
        if insurance_cover > 0 {
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
                insurance_cover,
            )?;
        }
        if reward_paid > 0 {
            token::transfer(
                CpiContext::new_with_signer(
                    token_program_ai.clone(),
                    Transfer {
                        from: collateral_vault_ai.clone(),
                        to: liquidator_usdc_ai,
                        authority: market_ai.clone(),
                    },
                    signer,
                ),
                reward_paid,
            )?;
        }
        if trader_refund > 0 {
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
                trader_refund,
            )?;
        }
    }

    let now = Clock::get()?.unix_timestamp;

    // Commit market state: position gone.
    {
        let market = &mut ctx.accounts.market;
        market.apply_reserves(new_base_res, new_quote_res);
        match side {
            crate::state::Side::Long => {
                market.long_open_notional = market.long_open_notional.saturating_sub(open_notional)
            }
            crate::state::Side::Short => {
                market.short_open_notional = market.short_open_notional.saturating_sub(open_notional)
            }
        }
        market.total_collateral = market.total_collateral.saturating_sub(old_margin);
        market.position_count = market.position_count.saturating_sub(1);
    }

    // Commit position state: flat.
    {
        let position = &mut ctx.accounts.position;
        position.base_size = 0;
        position.open_notional = 0;
        position.margin = 0;
        let pnl_i64 = i64::try_from(pnl).unwrap_or(if pnl < 0 { i64::MIN } else { i64::MAX });
        position.realized_pnl = position.realized_pnl.saturating_add(pnl_i64);
        position.last_updated = now;
    }

    emit!(PositionLiquidated {
        market: market_key,
        market_id,
        owner: owner_key,
        liquidator: liquidator_key,
        base_closed: abs_base,
        realized_pnl: i64::try_from(pnl).unwrap_or(if pnl < 0 { i64::MIN } else { i64::MAX }),
        reward: reward_paid,
        trader_refund,
        bad_debt,
        insurance_cover,
        mark_price: mark,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct LiquidatePosition<'info> {
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

    /// The owner's USDC account, to receive any residual equity.
    #[account(
        mut,
        constraint = owner_usdc.owner == position.owner,
        constraint = owner_usdc.mint == market.usdc_mint @ FuturesError::WrongMint
    )]
    pub owner_usdc: Box<Account<'info, TokenAccount>>,

    /// The liquidator's USDC account, to receive the keeper reward.
    #[account(
        mut,
        constraint = liquidator_usdc.owner == liquidator.key(),
        constraint = liquidator_usdc.mint == market.usdc_mint @ FuturesError::WrongMint
    )]
    pub liquidator_usdc: Box<Account<'info, TokenAccount>>,

    pub liquidator: Signer<'info>,

    pub token_program: Program<'info, Token>,
}
