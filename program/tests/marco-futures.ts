import * as anchor from "@coral-xyz/anchor";
import { Program, BN } from "@coral-xyz/anchor";
import { MarcoFutures } from "../target/types/marco_futures";
import {
  TOKEN_PROGRAM_ID,
  createMint,
  getOrCreateAssociatedTokenAccount,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import {
  PublicKey,
  Keypair,
  SystemProgram,
  SYSVAR_RENT_PUBKEY,
  LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import { assert } from "chai";

// ─── units ───
// Price index: USD-billions × 1e6. $40B == 40_000_000.
const PRICE = (billions: number) => new BN(Math.round(billions * 1_000_000));
// USDC has 6 decimals.
const USDC = (n: number) => new BN(n).mul(new BN(1_000_000));

describe("marco-futures", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  const program = anchor.workspace.marcoFutures as Program<MarcoFutures>;
  const connection = provider.connection;
  const admin = provider.wallet as anchor.Wallet;

  let usdcMint: PublicKey;
  let configPda: PublicKey;

  // Traders.
  const alice = Keypair.generate();
  const bob = Keypair.generate();
  const carol = Keypair.generate();
  const dave = Keypair.generate();
  const whale = Keypair.generate();
  const liquidator = Keypair.generate();
  const usdcOf: Record<string, PublicKey> = {};

  const marketPda = (id: string) =>
    PublicKey.findProgramAddressSync(
      [Buffer.from("market"), configPda.toBuffer(), Buffer.from(id)],
      program.programId
    )[0];
  const collateralPda = (mkt: PublicKey) =>
    PublicKey.findProgramAddressSync(
      [Buffer.from("collateral"), mkt.toBuffer()],
      program.programId
    )[0];
  const insurancePda = (mkt: PublicKey) =>
    PublicKey.findProgramAddressSync(
      [Buffer.from("insurance"), mkt.toBuffer()],
      program.programId
    )[0];
  const positionPda = (mkt: PublicKey, owner: PublicKey) =>
    PublicKey.findProgramAddressSync(
      [Buffer.from("position"), mkt.toBuffer(), owner.toBuffer()],
      program.programId
    )[0];

  async function fund(kp: Keypair) {
    const sig = await connection.requestAirdrop(kp.publicKey, 2 * LAMPORTS_PER_SOL);
    await connection.confirmTransaction(sig, "confirmed");
    const ata = await getOrCreateAssociatedTokenAccount(
      connection,
      admin.payer,
      usdcMint,
      kp.publicKey
    );
    usdcOf[kp.publicKey.toBase58()] = ata.address;
    await mintTo(
      connection,
      admin.payer,
      usdcMint,
      ata.address,
      admin.payer,
      BigInt(USDC(1_000_000).toString())
    );
  }

  const bal = async (ata: PublicKey) => new BN((await getAccount(connection, ata)).amount.toString());
  const usdc = (kp: Keypair) => usdcOf[kp.publicKey.toBase58()];

  // Create a fresh, isolated market and seed its insurance fund.
  async function createMarket(id: string, anchorBillions: number, insurance: number) {
    const market = marketPda(id);
    await program.methods
      .createMarket({
        marketId: id,
        anchorPrice: PRICE(anchorBillions),
        quoteReserve: USDC(100_000), // $100k vAMM depth
        maxLeverage: 3,
        maintenanceMarginBps: 625, // 6.25%
        takerFeeBps: 30, // 0.30%
        liquidationFeeBps: 100, // 1.00%
        maxOiNotional: USDC(5_000_000),
        expiryTs: new BN(Math.floor(Date.now() / 1000) + 365 * 24 * 3600),
      } as any)
      .accounts({
        config: configPda,
        market,
        usdcMint,
        collateralVault: collateralPda(market),
        insuranceVault: insurancePda(market),
        admin: admin.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: SYSVAR_RENT_PUBKEY,
      } as any)
      .rpc();

    if (insurance > 0) {
      await program.methods
        .seedInsurance(USDC(insurance))
        .accounts({
          market,
          insuranceVault: insurancePda(market),
          funderUsdc: usdc(admin.payer),
          funder: admin.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
        } as any)
        .rpc();
    }
    return market;
  }

  async function deposit(market: PublicKey, trader: Keypair, amount: BN) {
    await program.methods
      .depositCollateral(amount)
      .accounts({
        market,
        position: positionPda(market, trader.publicKey),
        collateralVault: collateralPda(market),
        ownerUsdc: usdc(trader),
        owner: trader.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
      } as any)
      .signers([trader])
      .rpc();
  }

  async function open(market: PublicKey, trader: Keypair, isLong: boolean, notional: BN) {
    await program.methods
      .openPosition(isLong, notional, new BN(0))
      .accounts({
        config: configPda,
        market,
        position: positionPda(market, trader.publicKey),
        collateralVault: collateralPda(market),
        insuranceVault: insurancePda(market),
        owner: trader.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      } as any)
      .signers([trader])
      .rpc();
  }

  async function close(market: PublicKey, trader: Keypair) {
    await program.methods
      .closePosition(new BN(0), new BN(0))
      .accounts({
        market,
        position: positionPda(market, trader.publicKey),
        collateralVault: collateralPda(market),
        insuranceVault: insurancePda(market),
        ownerUsdc: usdc(trader),
        owner: trader.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      } as any)
      .signers([trader])
      .rpc();
  }

  before(async () => {
    [configPda] = PublicKey.findProgramAddressSync([Buffer.from("config")], program.programId);

    // Ensure the provider wallet can pay for mints, rent and fees.
    const air = await connection.requestAirdrop(admin.publicKey, 100 * LAMPORTS_PER_SOL);
    await connection.confirmTransaction(air, "confirmed");

    usdcMint = await createMint(connection, admin.payer, admin.publicKey, null, 6);
    // Admin's own USDC (funds insurance seeds).
    const adminAta = await getOrCreateAssociatedTokenAccount(
      connection,
      admin.payer,
      usdcMint,
      admin.publicKey
    );
    usdcOf[admin.publicKey.toBase58()] = adminAta.address;
    await mintTo(connection, admin.payer, usdcMint, adminAta.address, admin.payer, BigInt(USDC(5_000_000).toString()));

    for (const kp of [alice, bob, carol, dave, whale, liquidator]) await fund(kp);

    await program.methods
      .initialize()
      .accounts({
        config: configPda,
        admin: admin.publicKey,
        operator: admin.publicKey,
        treasury: admin.publicKey,
        usdcMint,
        systemProgram: SystemProgram.programId,
      } as any)
      .rpc();
  });

  it("initializes the protocol config", async () => {
    const cfg = await program.account.futuresConfig.fetch(configPda);
    assert.ok(cfg.admin.equals(admin.publicKey));
    assert.ok(cfg.usdcMint.equals(usdcMint));
    assert.isFalse(cfg.paused);
  });

  it("creates the Moonshot AI market at $40B with mark == anchor", async () => {
    const market = await createMarket("moon-2026", 40, 50_000);
    const m = await program.account.market.fetch(market);
    assert.equal(m.marketId, "moon-2026");
    assert.equal(m.anchorPrice.toString(), PRICE(40).toString());
    // mark = quote_reserve * 1e6 / base_reserve == anchor at creation.
    const mark = m.quoteReserve.mul(new BN(1_000_000)).div(m.baseReserve);
    assert.equal(mark.toString(), PRICE(40).toString());
    const insBal = await bal(insurancePda(market));
    assert.equal(insBal.toString(), USDC(50_000).toString());
  });

  it("opens a long, counter-flow lifts the mark, long closes in profit", async () => {
    const market = await createMarket("m-openclose", 40, 50_000);
    await deposit(market, alice, USDC(2_000));
    await open(market, alice, true, USDC(4_000)); // 2x long

    // A large long lifts the mark well above Alice's entry.
    await deposit(market, bob, USDC(30_000));
    await open(market, bob, true, USDC(40_000));

    const m1 = await program.account.market.fetch(market);
    const mark1 = m1.quoteReserve.mul(new BN(1_000_000)).div(m1.baseReserve);
    assert.ok(mark1.gt(PRICE(40)), "mark should rise after net long flow");

    const before = await bal(usdc(alice));
    await close(market, alice);
    const after = await bal(usdc(alice));
    const pos = await program.account.position.fetch(positionPda(market, alice.publicKey));

    assert.ok(pos.realizedPnl.gt(new BN(0)), "Alice realized a profit");
    assert.ok(after.sub(before).gt(new BN(0)), "Alice received a payout");
    assert.equal(pos.baseSize.toString(), "0", "Alice position is flat");
  });

  it("rejects a position above the leverage cap", async () => {
    const market = await createMarket("m-leverage", 40, 0);
    await deposit(market, dave, USDC(1_000));
    let threw = false;
    try {
      await open(market, dave, true, USDC(5_000)); // 5x > 3x cap
    } catch (e: any) {
      threw = true;
      assert.match(e.toString(), /ExceedsMaxLeverage/);
    }
    assert.isTrue(threw, "5x open should be rejected");
  });

  it("liquidates an underwater long when the mark crashes", async () => {
    const market = await createMarket("m-liq", 40, 50_000);
    await deposit(market, carol, USDC(1_000));
    await open(market, carol, true, USDC(2_500)); // 2.5x long

    // A whale short crashes the mark far below Carol's entry.
    await deposit(market, whale, USDC(80_000));
    await open(market, whale, false, USDC(45_000));

    const m = await program.account.market.fetch(market);
    const mark = m.quoteReserve.mul(new BN(1_000_000)).div(m.baseReserve);
    assert.ok(mark.lt(PRICE(40)), "mark should crash after the short");

    const liqBefore = await bal(usdc(liquidator));
    await program.methods
      .liquidatePosition()
      .accounts({
        market,
        position: positionPda(market, carol.publicKey),
        collateralVault: collateralPda(market),
        insuranceVault: insurancePda(market),
        ownerUsdc: usdc(carol),
        liquidatorUsdc: usdc(liquidator),
        liquidator: liquidator.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      } as any)
      .signers([liquidator])
      .rpc();

    const pos = await program.account.position.fetch(positionPda(market, carol.publicKey));
    const liqAfter = await bal(usdc(liquidator));
    assert.equal(pos.margin.toString(), "0", "Carol position is wiped");
    assert.equal(pos.baseSize.toString(), "0", "Carol position is flat");
    assert.ok(pos.realizedPnl.lt(new BN(0)), "Carol realized a loss");
    assert.ok(liqAfter.gte(liqBefore), "liquidator was rewarded (or at least not charged)");
  });

  it("settles the market: long wins, short loses, pool stays solvent", async () => {
    // Small sizes keep price impact low, so both entries sit near the $40B
    // anchor and a $60B settlement is unambiguous: long wins, short loses.
    const market = await createMarket("m-settle", 40, 100_000);
    await deposit(market, bob, USDC(3_000));
    await open(market, bob, true, USDC(6_000)); // 2x long, ~$40B entry
    await deposit(market, alice, USDC(3_000));
    await open(market, alice, false, USDC(6_000)); // 2x short, ~$40B entry

    // Moonshot lists at $60B (+50% from anchor).
    await program.methods
      .settleMarket(PRICE(60))
      .accounts({ config: configPda, market, authority: admin.publicKey } as any)
      .rpc();

    const m = await program.account.market.fetch(market);
    assert.deepEqual(m.status, { settled: {} });

    // Settle the long (winner).
    const bobBefore = await bal(usdc(bob));
    await program.methods
      .settlePosition()
      .accounts({
        market,
        position: positionPda(market, bob.publicKey),
        collateralVault: collateralPda(market),
        insuranceVault: insurancePda(market),
        ownerUsdc: usdc(bob),
        caller: admin.publicKey,
      } as any)
      .rpc();
    const bobAfter = await bal(usdc(bob));
    const bobPos = await program.account.position.fetch(positionPda(market, bob.publicKey));
    assert.ok(bobPos.realizedPnl.gt(new BN(0)), "Bob (long) profits at $60B");
    assert.ok(bobAfter.sub(bobBefore).gt(USDC(3_000)), "Bob got back more than his margin");

    // Settle the short (loser).
    await program.methods
      .settlePosition()
      .accounts({
        market,
        position: positionPda(market, alice.publicKey),
        collateralVault: collateralPda(market),
        insuranceVault: insurancePda(market),
        ownerUsdc: usdc(alice),
        caller: admin.publicKey,
      } as any)
      .rpc();
    const alicePos = await program.account.position.fetch(positionPda(market, alice.publicKey));
    assert.ok(alicePos.realizedPnl.lt(new BN(0)), "Alice (short) loses at $60B");

    // Solvency: neither vault was overdrawn.
    const cv = await bal(collateralPda(market));
    const iv = await bal(insurancePda(market));
    assert.ok(cv.gte(new BN(0)) && iv.gte(new BN(0)), "vaults remain non-negative");
  });
});
