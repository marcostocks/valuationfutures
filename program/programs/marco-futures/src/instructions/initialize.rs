use anchor_lang::prelude::*;
use anchor_spl::token::Mint;

use crate::state::FuturesConfig;

/// Create the one protocol config for this deployment. Records the admin and
/// operator authorities and the single collateral mint (USDC). Per-market
/// economics are set later, at `create_market`.
pub fn handler(ctx: Context<Initialize>) -> Result<()> {
    let cfg = &mut ctx.accounts.config;
    cfg.bump = ctx.bumps.config;
    cfg.admin = ctx.accounts.admin.key();
    cfg.operator = ctx.accounts.operator.key();
    cfg.treasury = ctx.accounts.treasury.key();
    cfg.usdc_mint = ctx.accounts.usdc_mint.key();
    cfg.paused = false;
    cfg.market_count = 0;
    cfg._reserved = [0u8; 128];

    msg!(
        "FuturesConfig initialized | admin {} | operator {} | usdc {}",
        cfg.admin,
        cfg.operator,
        cfg.usdc_mint
    );
    Ok(())
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = admin,
        space = FuturesConfig::MAX_SIZE,
        seeds = [b"config"],
        bump
    )]
    pub config: Account<'info, FuturesConfig>,

    #[account(mut)]
    pub admin: Signer<'info>,

    /// CHECK: stored as a pubkey only.
    pub operator: AccountInfo<'info>,

    /// CHECK: stored as a pubkey only.
    pub treasury: AccountInfo<'info>,

    /// The collateral mint every market will accept (USDC, 6 decimals).
    pub usdc_mint: Account<'info, Mint>,

    pub system_program: Program<'info, System>,
}
