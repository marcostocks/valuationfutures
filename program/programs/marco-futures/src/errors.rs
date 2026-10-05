use anchor_lang::prelude::*;

#[error_code]
pub enum FuturesError {
    #[msg("Unauthorized — only the admin can perform this action")]
    UnauthorizedAdmin,

    #[msg("Unauthorized — only the admin or operator can perform this action")]
    UnauthorizedOperator,

    #[msg("Unauthorized — only the position owner can perform this action")]
    UnauthorizedOwner,

    #[msg("Protocol is paused")]
    ProtocolPaused,

    #[msg("Market is not in the required status for this operation")]
    InvalidMarketStatus,

    #[msg("Market id too long (max 32 bytes)")]
    MarketIdTooLong,

    #[msg("Invalid parameter supplied")]
    InvalidParameter,

    #[msg("Amount must be greater than zero")]
    ZeroAmount,

    #[msg("Requested size exceeds available vAMM liquidity")]
    InsufficientLiquidity,

    #[msg("Resulting leverage exceeds the market maximum")]
    ExceedsMaxLeverage,

    #[msg("Margin is insufficient for this position")]
    InsufficientMargin,

    #[msg("Open interest cap for this side would be exceeded")]
    OpenInterestCapExceeded,

    #[msg("Cannot flip sides in one instruction — close the position first")]
    WrongSide,

    #[msg("Position is flat — nothing to close")]
    PositionFlat,

    #[msg("Close size exceeds the open position")]
    CloseExceedsPosition,

    #[msg("Fill price is worse than the caller's limit")]
    SlippageExceeded,

    #[msg("Withdrawal would drop the position below its margin requirement")]
    WithdrawBelowMargin,

    #[msg("Nothing available to withdraw")]
    NothingToWithdraw,

    #[msg("Position is not liquidatable at the current mark")]
    NotLiquidatable,

    #[msg("Market is not settled yet")]
    MarketNotSettled,

    #[msg("Market is already settled")]
    MarketAlreadySettled,

    #[msg("Collateral vault does not match the market's collateral vault")]
    WrongCollateralVault,

    #[msg("Insurance vault does not match the market's insurance vault")]
    WrongInsuranceVault,

    #[msg("Token mint does not match the market's collateral mint")]
    WrongMint,

    #[msg("Arithmetic overflow")]
    MathOverflow,
}
