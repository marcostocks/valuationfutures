use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

use crate::errors::FuturesError;
use crate::state::Market;

/// Fund a market's insurance vault with USDC. Permissionless: the insurance
/// fund backstops trader payouts the collateral vault alone cannot cover, so
/// anyone (typically the protocol at launch) may strengthen it. Funds are not
/// tracked per depositor and are not withdrawable through this program in v1.
pub fn handler(ctx: Context<SeedInsurance>, amount: u64) -> Result<()> {
    require!(amount > 0, FuturesError::ZeroAmount);

    token::transfer(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.funder_usdc.to_account_info(),
                to: ctx.accounts.insurance_vault.to_account_info(),
                authority: ctx.accounts.funder.to_account_info(),
            },
        ),
        amount,
    )?;

    msg!(
        "Insurance seeded: market {} | +{} USDC",
        ctx.accounts.market.market_id,
        amount
    );
    Ok(())
}

#[derive(Accounts)]
pub struct SeedInsurance<'info> {
    #[account(
        seeds = [b"market", market.config.as_ref(), market.market_id.as_bytes()],
        bump = market.bump
    )]
    pub market: Box<Account<'info, Market>>,

    #[account(mut, constraint = insurance_vault.key() == market.insurance_vault @ FuturesError::WrongInsuranceVault)]
    pub insurance_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = funder_usdc.owner == funder.key(),
        constraint = funder_usdc.mint == market.usdc_mint @ FuturesError::WrongMint
    )]
    pub funder_usdc: Box<Account<'info, TokenAccount>>,

    #[account(mut)]
    pub funder: Signer<'info>,

    pub token_program: Program<'info, Token>,
}
