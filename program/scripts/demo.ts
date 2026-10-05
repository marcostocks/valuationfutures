/**
 * Narrated walkthrough of one Moonshot AI valuation future.
 *
 *   1. create the market at a $40B implied valuation
 *   2. a trader opens a 3x long with $5,000
 *   3. order flow lifts the mark — show the new implied valuation and PnL
 *   4. the trader closes for a profit
 *   5. a second market shows settlement: long into a $60B listing
 *
 * Run against a local validator (default) or any cluster via ANCHOR_PROVIDER_URL:
 *
 *   solana-test-validator            # in another terminal (or the repo's localnet)
 *   npm --prefix futures run demo
 *
 * Uses your Solana CLI wallet (~/.config/solana/id.json) as admin + trader A,
 * and mints a throwaway USDC for the demo.
 */
import * as anchor from "@coral-xyz/anchor";
import { BN } from "@coral-xyz/anchor";
import {
  TOKEN_PROGRAM_ID,
  createMint,
  getOrCreateAssociatedTokenAccount,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  SYSVAR_RENT_PUBKEY,
  LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import { readFileSync } from "fs";
import { homedir } from "os";
import { join } from "path";

const URL = process.env.ANCHOR_PROVIDER_URL || "http://127.0.0.1:8899";
const USDC = (n: number) => new BN(Math.round(n)).mul(new BN(1_000_000));
const PRICE = (billions: number) => new BN(Math.round(billions * 1_000_000));
const asB = (idx: BN) => `$${(idx.toNumber() / 1_000_000).toFixed(2)}B`;
const asUsd = (u: BN) => `$${(u.toNumber() / 1_000_000).toLocaleString(undefined, { maximumFractionDigits: 2 })}`;
const log = (s = "") => console.log(s);

async function main() {
  const walletPath = process.env.ANCHOR_WALLET || join(homedir(), ".config/solana/id.json");
  const walletKp = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(walletPath, "utf8"))));
  const connection = new Connection(URL, "confirmed");
  const provider = new anchor.AnchorProvider(connection, new anchor.Wallet(walletKp), {
    commitment: "confirmed",
  });
  const idl = JSON.parse(readFileSync(join(__dirname, "../target/idl/marco_futures.json"), "utf8"));
  // Untyped (`any`) so the script needs no generated types; the test suite is
  // the type-checked reference.
  const program: any = new anchor.Program(idl, provider);
  const pid: PublicKey = program.programId;

  log(`marco-futures demo → ${URL}`);
  log(`program ${pid.toBase58()}`);

  // Fund the admin/trader-A wallet on a local validator.
  if (URL.includes("127.0.0.1") || URL.includes("localhost")) {
    const sig = await connection.requestAirdrop(walletKp.publicKey, 100 * LAMPORTS_PER_SOL);
    await connection.confirmTransaction(sig, "confirmed");
  }

  const pda = (seeds: (Buffer | Uint8Array)[]) => PublicKey.findProgramAddressSync(seeds, pid)[0];
  const configPda = pda([Buffer.from("config")]);
  const market = (id: string) => pda([Buffer.from("market"), configPda.toBuffer(), Buffer.from(id)]);
  const collat = (m: PublicKey) => pda([Buffer.from("collateral"), m.toBuffer()]);
  const insur = (m: PublicKey) => pda([Buffer.from("insurance"), m.toBuffer()]);
  const posn = (m: PublicKey, o: PublicKey) => pda([Buffer.from("position"), m.toBuffer(), o.toBuffer()]);

  // Re-run safe: reuse the collateral mint if the protocol is already
  // initialised on this cluster, otherwise mint a throwaway USDC and init.
  let cfg: any = null;
  try {
    cfg = await program.account.futuresConfig.fetch(configPda);
  } catch {}
  const usdcMint: PublicKey = cfg
    ? cfg.usdcMint
    : await createMint(connection, walletKp, walletKp.publicKey, null, 6);

  const aUsdc = (await getOrCreateAssociatedTokenAccount(connection, walletKp, usdcMint, walletKp.publicKey)).address;
  await mintTo(connection, walletKp, usdcMint, aUsdc, walletKp, BigInt(USDC(1_000_000).toString()));

  const bob = Keypair.generate();
  await connection.confirmTransaction(await connection.requestAirdrop(bob.publicKey, 2 * LAMPORTS_PER_SOL), "confirmed");
  const bUsdc = (await getOrCreateAssociatedTokenAccount(connection, walletKp, usdcMint, bob.publicKey)).address;
  await mintTo(connection, walletKp, usdcMint, bUsdc, walletKp, BigInt(USDC(1_000_000).toString()));

  if (!cfg) {
    await program.methods
      .initialize()
      .accounts({
        config: configPda,
        admin: walletKp.publicKey,
        operator: walletKp.publicKey,
        treasury: walletKp.publicKey,
        usdcMint,
        systemProgram: SystemProgram.programId,
      })
      .rpc();
  }

  const markOf = async (m: PublicKey) => {
    const x = await program.account.market.fetch(m);
    return x.quoteReserve.mul(new BN(1_000_000)).div(x.baseReserve) as BN;
  };
  const makeMarket = async (id: string, insurance: number) => {
    const m = market(id);
    await program.methods
      .createMarket({
        marketId: id,
        anchorPrice: PRICE(40),
        quoteReserve: USDC(100_000),
        maxLeverage: 3,
        maintenanceMarginBps: 625,
        takerFeeBps: 30,
        liquidationFeeBps: 100,
        maxOiNotional: USDC(5_000_000),
        expiryTs: new BN(Math.floor(Date.now() / 1000) + 365 * 86400),
      })
      .accounts({
        config: configPda,
        market: m,
        usdcMint,
        collateralVault: collat(m),
        insuranceVault: insur(m),
        admin: walletKp.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: SYSVAR_RENT_PUBKEY,
      })
      .rpc();
    await program.methods
      .seedInsurance(USDC(insurance))
      .accounts({ market: m, insuranceVault: insur(m), funderUsdc: aUsdc, funder: walletKp.publicKey, tokenProgram: TOKEN_PROGRAM_ID })
      .rpc();
    return m;
  };
  const depositA = (m: PublicKey, amt: BN) =>
    program.methods.depositCollateral(amt).accounts({
      market: m, position: posn(m, walletKp.publicKey), collateralVault: collat(m), ownerUsdc: aUsdc,
      owner: walletKp.publicKey, tokenProgram: TOKEN_PROGRAM_ID, systemProgram: SystemProgram.programId,
    }).rpc();
  const openA = (m: PublicKey, isLong: boolean, notional: BN) =>
    program.methods.openPosition(isLong, notional, new BN(0)).accounts({
      config: configPda, market: m, position: posn(m, walletKp.publicKey), collateralVault: collat(m),
      insuranceVault: insur(m), owner: walletKp.publicKey, tokenProgram: TOKEN_PROGRAM_ID,
    }).rpc();

  const ts = Date.now();

  // ── Act 1: open a long, watch the implied valuation move ──
  log("\n── Moonshot AI · valuation future ──");
  const m1 = await makeMarket(`moon-demo-${ts}`, 50_000);
  log(`market created · implied valuation ${asB(await markOf(m1))}`);

  await depositA(m1, USDC(5_000));
  await openA(m1, true, USDC(9_000)); // ~1.8x long (headroom under the 3x cap)
  let p = await program.account.position.fetch(posn(m1, walletKp.publicKey));
  log(`\nTrader A · LONG · margin ${asUsd(p.margin as BN)} · notional ${asUsd(USDC(9_000))}`);
  log(`entry implied valuation ${asB(await markOf(m1))}`);

  // Bob adds long flow → the mark (and implied valuation) rises.
  await program.methods.depositCollateral(USDC(40_000)).accounts({
    market: m1, position: posn(m1, bob.publicKey), collateralVault: collat(m1), ownerUsdc: bUsdc,
    owner: bob.publicKey, tokenProgram: TOKEN_PROGRAM_ID, systemProgram: SystemProgram.programId,
  }).signers([bob]).rpc();
  await program.methods.openPosition(true, USDC(60_000), new BN(0)).accounts({
    config: configPda, market: m1, position: posn(m1, bob.publicKey), collateralVault: collat(m1),
    insuranceVault: insur(m1), owner: bob.publicKey, tokenProgram: TOKEN_PROGRAM_ID,
  }).signers([bob]).rpc();

  const mark2 = await markOf(m1);
  p = await program.account.position.fetch(posn(m1, walletKp.publicKey));
  const uPnl = (p.baseSize as BN).abs().mul(mark2).div(new BN(1_000_000)).sub(p.openNotional as BN);
  log(`\norder flow lifts the market → implied valuation ${asB(mark2)}`);
  log(`Trader A unrealized PnL ≈ ${asUsd(uPnl)}`);

  const aBefore = new BN((await getAccount(connection, aUsdc)).amount.toString());
  await program.methods.closePosition(new BN(0), new BN(0)).accounts({
    market: m1, position: posn(m1, walletKp.publicKey), collateralVault: collat(m1), insuranceVault: insur(m1),
    ownerUsdc: aUsdc, owner: walletKp.publicKey, tokenProgram: TOKEN_PROGRAM_ID,
  }).rpc();
  const aAfter = new BN((await getAccount(connection, aUsdc)).amount.toString());
  p = await program.account.position.fetch(posn(m1, walletKp.publicKey));
  log(`Trader A closes · payout ${asUsd(aAfter.sub(aBefore))} · realized PnL ${asUsd(p.realizedPnl as BN)}`);

  // ── Act 2: settlement at the IPO ──
  log("\n── settlement · Moonshot lists at $60B ──");
  const m2 = await makeMarket(`moon-settle-${ts}`, 100_000);
  await depositA(m2, USDC(5_000));
  await openA(m2, true, USDC(8_000)); // ~1.6x long near $40B
  log(`Trader A · LONG · entry ${asB(await markOf(m2))}`);

  await program.methods.settleMarket(PRICE(60)).accounts({ config: configPda, market: m2, authority: walletKp.publicKey }).rpc();
  log(`market settled at $60.00B`);

  const sBefore = new BN((await getAccount(connection, aUsdc)).amount.toString());
  await program.methods.settlePosition().accounts({
    market: m2, position: posn(m2, walletKp.publicKey), collateralVault: collat(m2), insuranceVault: insur(m2),
    ownerUsdc: aUsdc, caller: walletKp.publicKey,
  }).rpc();
  const sAfter = new BN((await getAccount(connection, aUsdc)).amount.toString());
  const sp = await program.account.position.fetch(posn(m2, walletKp.publicKey));
  log(`Trader A settles · payout ${asUsd(sAfter.sub(sBefore))} · realized PnL ${asUsd(sp.realizedPnl as BN)}`);
  log("\ndone.");
}

main().then(
  () => process.exit(0),
  (e) => {
    console.error(e);
    process.exit(1);
  }
);
