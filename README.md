# Marco Valuation Futures

**Leveraged long/short exposure on the valuation of private, pre-IPO companies —
cash-settled at the IPO.**

Live demo → **https://marcostocks.github.io/valuationfutures/**

---

## What are Valuation Futures?

Some of the most valuable companies in the world are still private — OpenAI,
ByteDance, Moonshot AI, Anthropic, SpaceX. Yet almost no one can trade them: their
shares are illiquid, locked up, restricted to insiders and employees, and there is
**no way to go short or hedge** a pre-IPO position. Price discovery happens in
infrequent, opaque funding rounds months apart.

A **Valuation Future** fixes that. It is a **dated futures contract on a company's
total enterprise valuation**. You take a leveraged **long** (valuation goes up) or
**short** (valuation goes down) position on *what the company will be worth*, and the
contract **cash-settles in USDC at the real liquidity event** — the IPO (or an
acquisition). No shares change hands; you are trading the valuation itself.

**Why it's different from a per-share contract — dilution-proof pricing.**
Private companies raise across many rounds with new share classes, option pools and
anti-dilution terms, so a *per-share* contract breaks every time the cap table
changes. Marco prices the **total valuation** instead:

> **Total Valuation = Share Price × Fully-Diluted Shares Outstanding**

quoted in points where **1 point = $1B**. A $40B contract is still a $40B contract
regardless of interim rounds — only the IPO prospectus converts it to per-share units.

**At a glance**

| | |
|---|---|
| Underlying | A company's total enterprise valuation at IPO (not per-share) |
| Unit | 1 point = $1B USD |
| Direction | Long or short, up to **10× leverage** (isolated margin) |
| Hours | 24 / 7 / 365 |
| Settlement | Cash (USDC) vs. the **IPO first-day close** |
| Collateral | USDC, held in a program-owned vault on Solana |

---

## How it works

**Mark price — internal by default, external by override.**
Private equity has no continuous spot feed, so the mark is built from two sources: an
**internal** 8-hour EMA of the order book (the market's own live estimate of the
valuation), and an **external** anchor — the last *verified* hard event (a priced
funding round, a qualifying secondary, a corporate action). The internal EMA is the
mark in normal operation; when a hard external event is confirmed, the mark
**converges** to the new anchor over a 24–48h window (not a jump), preventing
liquidation cascades.

**Margin, leverage & liquidation.**
Positions use **isolated margin** (collateral is ring-fenced to one position). Initial
margin as low as **10%** (up to 10×); if account equity falls below the maintenance
margin, the position is **liquidated permissionlessly**. A per-market **insurance
fund**, seeded permissionlessly and fed by trading fees, backstops residual skew.

**The venue is the counterparty.**
Unlike a spot vault, this program holds trader margin and takes the other side, so it
is built around two invariants: **solvency by construction** (no instruction ever pays
out more than a vault holds) and **per-market isolation** (margin, reserves and
insurance never cross markets).

**Settlement & fallbacks.**
It is a *dated* future, so it resolves at the catalyst rather than running forever:

| Outcome | Settles at |
|---|---|
| **IPO (standard)** | Rebase to per-share at the prospectus → **IPO first-day close** |
| IPO delayed > 24 months | NAV via three independent valuation firms |
| M&A / acquisition | Disclosed acquisition enterprise value |
| Direct listing | The reference price |
| IPO withdrawn | Contract voided; initial margin returned |

---

## What's in this repo

| Path | What it is |
|---|---|
| **`index.html`** | The trading terminal — a single self-contained web app (one market, Moonshot AI). Mark chart, order ticket, live position & PnL, settlement countdown, shareable PnL card. Runs entirely client-side on simulated data; no server or dependencies. |
| **`how-it-works.html`** | Technical / due-diligence overview — architecture, mark price, risk & solvency, settlement, and a code map. |
| **`program/`** | The on-chain program (`marco-futures`) — Solana / Anchor / Rust. |

**View the demo:** open **https://marcostocks.github.io/valuationfutures/**, or
download `index.html` and open it locally. (Viewing an `.html` file directly on
github.com shows source, not the running page.)

---

## The on-chain program (`program/`)

`marco-futures` — a purpose-built **Anchor** program (Solana, Rust). The program is the
counterparty: trader USDC margin lives in a program-owned vault, a per-market insurance
fund backstops residual skew, and positions settle at an admin-posted price at the IPO.

- **Framework:** Anchor 0.31.1 · Rust 2021 · `overflow-checks` on
- **Surface:** 13 instructions · 3 account types (`FuturesConfig`, `Market`, `Position`) · 7 events · 25 typed errors
- **Design:** isolated margin, vAMM mark, permissionless liquidation, solvency-by-construction
- **Program ID:** `GCW6Gt86tSVMqEwz6GkVNDWuzivDXCG2bzjp4AS3ztkW`
- **Live on Solana devnet** — [view on Explorer](https://explorer.solana.com/address/GCW6Gt86tSVMqEwz6GkVNDWuzivDXCG2bzjp4AS3ztkW?cluster=devnet) (upgradeable; the deployer wallet holds upgrade authority)

### Instruction set
- **Governance** (admin / operator): `initialize`, `create_market`, `settle_market`, `pause`, `set_operator`, `halt_market`
- **Trader** (holder-signed): `deposit_collateral`, `withdraw_collateral`, `open_position`, `close_position`
- **Permissionless:** `seed_insurance`, `liquidate_position`, `settle_position`

### Build & test
```bash
cd program
anchor build      # compile the program + IDL
anchor test       # run the integration test suite (local validator)
```

> Build artifacts (`target/`), dependencies (`node_modules/`), the local validator
> state (`.anchor/`), and all keypairs are intentionally excluded (see `.gitignore`).

---

*Illustrative / simulated. The demo uses simulated data; nothing here is connected to
real funds or live market prices. This material is informational only and does not
constitute an offer or solicitation to purchase any security.*
